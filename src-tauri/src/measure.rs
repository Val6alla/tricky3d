//! Measure mode's geometry (src/measure): turning a click on a part into a feature (a snapped
//! point, a straight edge, a circle found in the mesh edges, a flat surface, a wall's thickness),
//! fitting 3-point circles and 4-point spheres, and the distances, angles, radii and positions
//! between features. Plain functions over meshes, tested against shapes whose measures are known.
//!
//! Features are stored in the part-local frame of the part a measurement belongs to, so it moves,
//! turns and scales with the part; `evaluate` takes them to world space with the part's matrix
//! first. A circle or sphere goes through the matrix as the points that define it, so it stays
//! exact under any rotation and uniform scale.

use std::f64::consts::PI;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::mesh::{Matrix, Mesh, cross, dot, sub, transform_point};
use crate::repair::bvh::{Bvh, triangle_boxes};
use crate::repair::topology::{Edges, NONE, bbox_diagonal};

/// Two triangles meeting at more than this angle (degrees) make a real edge: the edges snapping,
/// edge picks and circle detection follow.
pub const FEATURE_ANGLE: f64 = 20.0;

/// Neighbouring triangles count as one flat surface while their normals stay this close (degrees)
/// to the clicked one's.
pub const SURFACE_ANGLE: f64 = 1.0;

/// Edges whose directions differ by less than this (degrees) continue one straight edge.
const STRAIGHT_ANGLE: f64 = 1.5;

/// A detected circle's points may stray this share of its radius from it (root mean square).
const CIRCLE_FIT: f64 = 0.01;

/// Lines this close to parallel (degrees) have a distance between them; others get the shortest
/// distance between them with a note.
const PARALLEL_ANGLE: f64 = 0.1;

pub type V3 = [f64; 3];

/// Something measured on a part. Field names match `MeasureFeature` in src/measure/types.ts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Feature {
    Point {
        p: V3,
    },
    /// A straight line through `a` and `b` (the edge's ends, for drawing).
    Edge {
        a: V3,
        b: V3,
    },
    Circle {
        centre: V3,
        normal: V3,
        radius: f64,
    },
    Sphere {
        centre: V3,
        radius: f64,
    },
    /// A flat surface: a point on it (its centre) and its outward normal.
    Surface {
        point: V3,
        normal: V3,
    },
    /// A wall's thickness: from the clicked point straight through the material to the other side.
    Wall {
        from: V3,
        to: V3,
    },
}

/// How a clicked point snaps. Names match `SnapMode` in src/measure/types.ts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Snap {
    /// A corner when one is near, else a real edge when one is near, else the point clicked.
    Auto,
    Free,
    Edge,
    Node,
    /// Where a clip plane cuts the surface.
    ClipLine,
    /// Where a clip plane crosses a real edge.
    ClipCorner,
}

/// What a click picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PickKind {
    Point,
    Edge,
    Circle,
    Surface,
    Wall,
}

/// A plane in three.js form, world space: it keeps the points where dot(normal, p) + constant >= 0.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct ClipPlane {
    pub normal: V3,
    pub constant: f64,
}

// ---------------------------------------------------------------------------------------------
// Vectors and matrices

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn length(a: V3) -> f64 {
    dot(a, a).sqrt()
}

fn unit(a: V3) -> Option<V3> {
    let l = length(a);
    (l > 0.0 && l.is_finite()).then(|| scale(a, 1.0 / l))
}

fn distance(a: V3, b: V3) -> f64 {
    length(sub(a, b))
}

/// Inverse of an affine matrix (column-major), None when it is singular.
pub fn invert(m: &Matrix) -> Option<Matrix> {
    let [a, b, c] = [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]];
    let det = dot(a, cross(b, c));
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    // Rows of the inverse of the 3×3 whose columns are a, b, c.
    let r = [cross(b, c), cross(c, a), cross(a, b)].map(|v| scale(v, 1.0 / det));
    let t = [m[12], m[13], m[14]];
    let mut out = [0.0; 16];
    for (row, v) in r.iter().enumerate() {
        out[row] = v[0];
        out[4 + row] = v[1];
        out[8 + row] = v[2];
        out[12 + row] = -dot(*v, t);
    }
    out[15] = 1.0;
    Some(out)
}

/// A normal under `m`: by the cofactor matrix, which keeps it at right angles to the surface.
fn transform_normal(m: &Matrix, n: V3) -> V3 {
    let [a, b, c] = [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]];
    let k = [cross(b, c), cross(c, a), cross(a, b)];
    let v = add(add(scale(k[0], n[0]), scale(k[1], n[1])), scale(k[2], n[2]));
    let flip = if dot(a, cross(b, c)) < 0.0 { -1.0 } else { 1.0 };
    unit(scale(v, flip)).unwrap_or(n)
}

/// Two unit vectors at right angles to `n` and to each other.
fn basis(n: V3) -> (V3, V3) {
    let pick = if n[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let u = unit(cross(n, pick)).unwrap_or([1.0, 0.0, 0.0]);
    (u, cross(n, u))
}

/// A feature carried through `m` (part-local to world, or from one part's frame to another's).
pub fn transform(f: &Feature, m: &Matrix) -> Result<Feature, String> {
    let p = |v: V3| transform_point(m, v);
    Ok(match *f {
        Feature::Point { p: v } => Feature::Point { p: p(v) },
        Feature::Edge { a, b } => Feature::Edge { a: p(a), b: p(b) },
        Feature::Circle {
            centre,
            normal,
            radius,
        } => {
            let (u, v) = basis(normal);
            let points =
                [scale(u, radius), scale(v, radius), scale(u, -radius)].map(|d| p(add(centre, d)));
            let Feature::Circle { centre, radius, .. } = circle_through(points)? else {
                unreachable!()
            };
            Feature::Circle {
                centre,
                normal: transform_normal(m, normal),
                radius,
            }
        }
        Feature::Sphere { centre, radius } => {
            let points = [
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [-1.0, 0.0, 0.0],
            ]
            .map(|d| p(add(centre, scale(d, radius))));
            sphere_through(points)?
        }
        Feature::Surface { point, normal } => Feature::Surface {
            point: p(point),
            normal: transform_normal(m, normal),
        },
        Feature::Wall { from, to } => Feature::Wall {
            from: p(from),
            to: p(to),
        },
    })
}

// ---------------------------------------------------------------------------------------------
// Fits

/// The circle through three points.
pub fn circle_through([a, b, c]: [V3; 3]) -> Result<Feature, String> {
    let (ab, ac) = (sub(b, a), sub(c, a));
    let n = cross(ab, ac);
    let nn = dot(n, n);
    let size = length(ab).max(length(ac));
    if nn <= (1e-12 * size * size).powi(2) || !nn.is_finite() {
        return Err("The three points lie on one line, so no circle passes through them.".into());
    }
    // Circumcentre: a + (|ac|² (n × ab) + |ab|² (ac × n)) / (2 |n|²).
    let offset = scale(
        add(
            scale(cross(n, ab), dot(ac, ac)),
            scale(cross(ac, n), dot(ab, ab)),
        ),
        0.5 / nn,
    );
    let centre = add(a, offset);
    Ok(Feature::Circle {
        centre,
        normal: scale(n, 1.0 / nn.sqrt()),
        radius: length(offset),
    })
}

/// The sphere through four points.
pub fn sphere_through([a, b, c, d]: [V3; 4]) -> Result<Feature, String> {
    // |x − a|² = |x − p|² for p = b, c, d: three linear equations 2 (p − a)·x = |p|² − |a|².
    let rows = [b, c, d].map(|p| (scale(sub(p, a), 2.0), dot(p, p) - dot(a, a)));
    let [r0, r1, r2] = rows.map(|r| r.0);
    let det = dot(r0, cross(r1, r2));
    let size = [b, c, d]
        .iter()
        .map(|p| distance(*p, a))
        .fold(0.0, f64::max);
    if det.abs() <= 1e-12 * size.powi(3) || !det.is_finite() {
        return Err("The four points lie in one plane, so no sphere passes through them.".into());
    }
    let inv = [cross(r1, r2), cross(r2, r0), cross(r0, r1)];
    let rhs = rows.map(|r| r.1);
    let centre = scale(
        add(
            add(scale(inv[0], rhs[0]), scale(inv[1], rhs[1])),
            scale(inv[2], rhs[2]),
        ),
        1.0 / det,
    );
    Ok(Feature::Sphere {
        centre,
        radius: distance(centre, a),
    })
}

/// Unit eigenvector of the smallest eigenvalue of a symmetric 3×3 matrix (Jacobi rotations).
fn smallest_eigenvector(mut a: [[f64; 3]; 3]) -> V3 {
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..50 {
        let off = a[0][1].abs() + a[0][2].abs() + a[1][2].abs();
        if off < 1e-30 {
            break;
        }
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            if a[p][q].abs() < 1e-300 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
            let t = if theta == 0.0 { 1.0 } else { t };
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            for row in &mut a {
                let (akp, akq) = (row[p], row[q]);
                row[p] = c * akp - s * akq;
                row[q] = s * akp + c * akq;
            }
            let (row_p, row_q) = (a[p], a[q]);
            for k in 0..3 {
                a[p][k] = c * row_p[k] - s * row_q[k];
                a[q][k] = s * row_p[k] + c * row_q[k];
            }
            for row in &mut v {
                let (vp, vq) = (row[p], row[q]);
                row[p] = c * vp - s * vq;
                row[q] = s * vp + c * vq;
            }
        }
    }
    let i = (0..3).min_by(|&i, &j| a[i][i].total_cmp(&a[j][j])).unwrap();
    unit([v[0][i], v[1][i], v[2][i]]).unwrap_or([0.0, 0.0, 1.0])
}

/// The least-squares circle through points that lie roughly on one: plane by principal axes, then
/// the algebraic (Kåsa) fit in it. With the root-mean-square distance of the points from it.
fn fit_circle(points: &[V3]) -> Option<(Feature, f64)> {
    if points.len() < 3 {
        return None;
    }
    let n = points.len() as f64;
    let mean = scale(points.iter().fold([0.0; 3], |s, p| add(s, *p)), 1.0 / n);
    let mut cov = [[0.0; 3]; 3];
    for p in points {
        let d = sub(*p, mean);
        for i in 0..3 {
            for j in 0..3 {
                cov[i][j] += d[i] * d[j];
            }
        }
    }
    let normal = smallest_eigenvector(cov);
    let (u, v) = basis(normal);
    // Kåsa: minimise Σ (x² + y² + D x + E y + F)², a 3×3 linear system.
    let mut m = [[0.0; 3]; 3];
    let mut r = [0.0; 3];
    for p in points {
        let d = sub(*p, mean);
        let (x, y) = (dot(d, u), dot(d, v));
        let row = [x, y, 1.0];
        let z = -(x * x + y * y);
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += row[i] * row[j];
            }
            r[i] += row[i] * z;
        }
    }
    let det = dot(m[0], cross(m[1], m[2]));
    if det.abs() < 1e-300 || !det.is_finite() {
        return None;
    }
    let inv = [cross(m[1], m[2]), cross(m[2], m[0]), cross(m[0], m[1])];
    let sol = scale(
        add(
            add(scale(inv[0], r[0]), scale(inv[1], r[1])),
            scale(inv[2], r[2]),
        ),
        1.0 / det,
    );
    let (cx, cy) = (-sol[0] / 2.0, -sol[1] / 2.0);
    let r2 = cx * cx + cy * cy - sol[2];
    if r2.is_nan() || r2 <= 0.0 {
        return None;
    }
    let radius = r2.sqrt();
    let centre = add(mean, add(scale(u, cx), scale(v, cy)));
    let rms = (points
        .iter()
        .map(|p| {
            let d = sub(*p, centre);
            let h = dot(d, normal);
            let rho = length(sub(d, scale(normal, h)));
            h * h + (rho - radius).powi(2)
        })
        .sum::<f64>()
        / n)
        .sqrt();
    Some((
        Feature::Circle {
            centre,
            normal,
            radius,
        },
        rms,
    ))
}

// ---------------------------------------------------------------------------------------------
// A part's mesh, ready for picking

/// A part-local mesh with what picking needs: its edges, triangle normals, which edges are real
/// edges, the edges at each vertex, and (built on first use) a box tree for ray casts.
pub struct MeshIndex {
    pub mesh: Mesh,
    edges: Edges,
    normals: Vec<V3>,
    feature: Vec<bool>,
    /// The edges at each vertex: `vertex_edges[vertex_start[v]..vertex_start[v + 1]]`.
    vertex_start: Vec<u32>,
    vertex_edges: Vec<u32>,
    bvh: OnceLock<Bvh>,
    diagonal: f64,
}

impl MeshIndex {
    pub fn new(mesh: Mesh) -> MeshIndex {
        let edges = Edges::new(&mesh);
        let normals: Vec<V3> = mesh
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let (a, b, c) = (mesh.point(t[0]), mesh.point(t[1]), mesh.point(t[2]));
                unit(cross(sub(b, a), sub(c, a))).unwrap_or([0.0; 3])
            })
            .collect();
        let cos = FEATURE_ANGLE.to_radians().cos();
        let feature: Vec<bool> = (0..edges.len() as u32)
            .map(|e| {
                let sides = edges.sides(e);
                sides.len() != 2
                    || dot(
                        normals[sides[0] as usize / 3],
                        normals[sides[1] as usize / 3],
                    ) < cos
            })
            .collect();
        let count = mesh.vertex_count();
        let mut vertex_start = vec![0u32; count + 1];
        for e in 0..edges.len() as u32 {
            for v in edges.ends(e) {
                vertex_start[v as usize + 1] += 1;
            }
        }
        for v in 0..count {
            vertex_start[v + 1] += vertex_start[v];
        }
        let mut fill = vertex_start[..count].to_vec();
        let mut vertex_edges = vec![0u32; vertex_start[count] as usize];
        for e in 0..edges.len() as u32 {
            for v in edges.ends(e) {
                vertex_edges[fill[v as usize] as usize] = e;
                fill[v as usize] += 1;
            }
        }
        let diagonal = bbox_diagonal(&mesh).max(1e-9);
        MeshIndex {
            mesh,
            edges,
            normals,
            feature,
            vertex_start,
            vertex_edges,
            bvh: OnceLock::new(),
            diagonal,
        }
    }

    fn point(&self, v: u32) -> V3 {
        self.mesh.point(v)
    }

    fn edges_at(&self, v: u32) -> &[u32] {
        &self.vertex_edges
            [self.vertex_start[v as usize] as usize..self.vertex_start[v as usize + 1] as usize]
    }

    fn corners(&self, t: u32) -> [u32; 3] {
        let i = t as usize * 3;
        [
            self.mesh.indices[i],
            self.mesh.indices[i + 1],
            self.mesh.indices[i + 2],
        ]
    }

    fn check_triangle(&self, t: u32) -> Result<(), String> {
        if (t as usize) < self.normals.len() {
            Ok(())
        } else {
            Err("The click did not land on the part's surface.".into())
        }
    }

    /// The edges near triangle `t`: those at its corners and at their neighbours' far ends.
    fn nearby_edges(&self, t: u32) -> Vec<u32> {
        let mut out: Vec<u32> = Vec::new();
        for v in self.corners(t) {
            for &e in self.edges_at(v) {
                out.push(e);
                for w in self.edges.ends(e) {
                    out.extend_from_slice(self.edges_at(w));
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The real edge nearest to `p` among the edges near `t`, with the nearest point on it.
    fn nearest_feature_edge(&self, t: u32, p: V3) -> Option<(u32, V3)> {
        self.nearby_edges(t)
            .into_iter()
            .filter(|&e| self.feature[e as usize])
            .map(|e| {
                let [a, b] = self.edges.ends(e).map(|v| self.point(v));
                (e, closest_on_segment(p, a, b))
            })
            .min_by(|x, y| distance(p, x.1).total_cmp(&distance(p, y.1)))
    }

    fn bvh(&self) -> &Bvh {
        self.bvh
            .get_or_init(|| Bvh::new(&triangle_boxes(&self.mesh)))
    }
}

fn closest_on_segment(p: V3, a: V3, b: V3) -> V3 {
    let d = sub(b, a);
    let dd = dot(d, d);
    if dd == 0.0 {
        return a;
    }
    add(a, scale(d, (dot(sub(p, a), d) / dd).clamp(0.0, 1.0)))
}

/// The meshes Measure mode clicks on, kept between clicks under numbers the window gives them: the
/// window sends a mesh once, not with every click. A few at a time, the latest used last.
#[derive(Default)]
pub struct MeasureCache(Mutex<Vec<(u64, Arc<MeshIndex>)>>);

/// Meshes the cache keeps.
const CACHED: usize = 4;

impl MeasureCache {
    pub fn get(&self, key: u64) -> Option<Arc<MeshIndex>> {
        let mut held = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let i = held.iter().position(|(k, _)| *k == key)?;
        let entry = held.remove(i);
        let index = entry.1.clone();
        held.push(entry);
        Some(index)
    }

    pub fn put(&self, key: u64, index: Arc<MeshIndex>) {
        let mut held = self.0.lock().unwrap_or_else(|p| p.into_inner());
        held.retain(|(k, _)| *k != key);
        held.push((key, index));
        if held.len() > CACHED {
            held.remove(0);
        }
    }

    /// `measure_reset`: lets go of every mesh (Measure mode closed).
    pub fn clear(&self) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
}

// ---------------------------------------------------------------------------------------------
// Picks (part-local)

/// A click on triangle `triangle` at `hit`, both in the part's own frame.
pub struct Click<'a> {
    pub triangle: u32,
    pub hit: V3,
    pub snap: Snap,
    /// How far (mm, part frame) a corner or edge may be from the click and still be snapped to.
    pub tolerance: f64,
    /// The clip planes in force, in the part's frame.
    pub clip: &'a [ClipPlane],
}

/// The point a click picks, snapped as asked.
pub fn snap_point(index: &MeshIndex, click: &Click) -> Result<V3, String> {
    let t = click.triangle;
    index.check_triangle(t)?;
    let hit = click.hit;
    let node = || {
        let mut candidates: Vec<u32> = index.corners(t).to_vec();
        for e in index
            .nearby_edges(t)
            .into_iter()
            .filter(|&e| index.feature[e as usize])
        {
            candidates.extend(index.edges.ends(e));
        }
        candidates
            .into_iter()
            .map(|v| index.point(v))
            .min_by(|a, b| distance(hit, *a).total_cmp(&distance(hit, *b)))
            .expect("a triangle has corners")
    };
    let edge = || {
        index
            .nearest_feature_edge(t, hit)
            .map(|(_, p)| p)
            .unwrap_or_else(|| {
                let [a, b, c] = index.corners(t).map(|v| index.point(v));
                [
                    closest_on_segment(hit, a, b),
                    closest_on_segment(hit, b, c),
                    closest_on_segment(hit, c, a),
                ]
                .into_iter()
                .min_by(|x, y| distance(hit, *x).total_cmp(&distance(hit, *y)))
                .unwrap()
            })
    };
    match click.snap {
        Snap::Free => Ok(hit),
        Snap::Node => Ok(node()),
        Snap::Edge => Ok(edge()),
        Snap::Auto => {
            let corner = node();
            if distance(hit, corner) <= click.tolerance {
                return Ok(corner);
            }
            match index.nearest_feature_edge(t, hit) {
                Some((_, p)) if distance(hit, p) <= click.tolerance => Ok(p),
                _ => Ok(hit),
            }
        }
        Snap::ClipLine | Snap::ClipCorner => {
            let plane = click
                .clip
                .iter()
                .min_by(|a, b| plane_distance(a, hit).total_cmp(&plane_distance(b, hit)))
                .ok_or("Turn on a clip plane to snap to where it cuts the part.")?;
            let found = if click.snap == Snap::ClipLine {
                clip_line_point(index, t, hit, plane)
            } else {
                clip_corner_point(index, t, hit, plane)
            };
            found.ok_or_else(|| "Click closer to where the clip plane cuts the part.".into())
        }
    }
}

fn plane_side(plane: &ClipPlane, p: V3) -> f64 {
    dot(plane.normal, p) + plane.constant
}

fn plane_distance(plane: &ClipPlane, p: V3) -> f64 {
    plane_side(plane, p).abs() / length(plane.normal).max(1e-300)
}

/// Where segment a–b crosses the plane, if it does.
fn crossing(plane: &ClipPlane, a: V3, b: V3) -> Option<V3> {
    let (sa, sb) = (plane_side(plane, a), plane_side(plane, b));
    if sa == sb || sa.signum() == sb.signum() && sa != 0.0 && sb != 0.0 {
        return None;
    }
    let s = sa / (sa - sb);
    Some(add(a, scale(sub(b, a), s)))
}

/// The nearest point to `hit` on the line where the plane cuts the triangles near `t`.
fn clip_line_point(index: &MeshIndex, t: u32, hit: V3, plane: &ClipPlane) -> Option<V3> {
    let mut triangles: Vec<u32> = vec![t];
    for e in index.nearby_edges(t) {
        triangles.extend(index.edges.sides(e).iter().map(|s| s / 3));
    }
    triangles.sort_unstable();
    triangles.dedup();
    triangles
        .into_iter()
        .filter_map(|tri| {
            let [a, b, c] = index.corners(tri).map(|v| index.point(v));
            let cuts: Vec<V3> = [(a, b), (b, c), (c, a)]
                .into_iter()
                .filter_map(|(p, q)| crossing(plane, p, q))
                .collect();
            (cuts.len() >= 2).then(|| closest_on_segment(hit, cuts[0], cuts[1]))
        })
        .min_by(|x, y| distance(hit, *x).total_cmp(&distance(hit, *y)))
}

/// The nearest point to `hit` where the plane crosses a real edge near `t`.
fn clip_corner_point(index: &MeshIndex, t: u32, hit: V3, plane: &ClipPlane) -> Option<V3> {
    index
        .nearby_edges(t)
        .into_iter()
        .filter(|&e| index.feature[e as usize])
        .filter_map(|e| {
            let [a, b] = index.edges.ends(e).map(|v| index.point(v));
            crossing(plane, a, b)
        })
        .min_by(|x, y| distance(hit, *x).total_cmp(&distance(hit, *y)))
}

/// The straight edge a click picks: the real edge nearest the click (or the nearest side of the
/// clicked triangle), carried on along edges that keep its direction.
pub fn pick_edge(index: &MeshIndex, triangle: u32, hit: V3) -> Result<Feature, String> {
    index.check_triangle(triangle)?;
    let start = match index.nearest_feature_edge(triangle, hit) {
        Some((e, _)) => e,
        None => {
            let corners = index.corners(triangle);
            (0..3)
                .map(|k| index.edges.edge_of(triangle * 3 + k))
                .filter(|&e| e != NONE)
                .min_by(|&x, &y| {
                    let d = |e: u32| {
                        let [a, b] = index.edges.ends(e).map(|v| index.point(v));
                        distance(hit, closest_on_segment(hit, a, b))
                    };
                    d(x).total_cmp(&d(y))
                })
                .ok_or_else(|| format!("The triangle {corners:?} has no edges."))?
        }
    };
    let [u, v] = index.edges.ends(start);
    let dir = unit(sub(index.point(v), index.point(u))).ok_or("That edge has no length.")?;
    let cos = STRAIGHT_ANGLE.to_radians().cos();
    // Walks from `from` away from `back` while a real edge carries on in direction `d`.
    let walk = |mut from: u32, mut back: u32, d: V3| {
        let mut steps = 0;
        loop {
            let next = index
                .edges_at(from)
                .iter()
                .copied()
                .filter(|&e| e != back && index.feature[e as usize])
                .find_map(|e| {
                    let [a, b] = index.edges.ends(e);
                    let to = if a == from { b } else { a };
                    let step = unit(sub(index.point(to), index.point(from)))?;
                    (dot(step, d) >= cos).then_some((e, to))
                });
            match next {
                Some((e, to)) if steps < 1_000_000 => {
                    back = e;
                    from = to;
                    steps += 1;
                }
                _ => return from,
            }
        }
    };
    let (a, b) = if index.feature[start as usize] {
        (walk(u, start, scale(dir, -1.0)), walk(v, start, dir))
    } else {
        (u, v)
    };
    Ok(Feature::Edge {
        a: index.point(a),
        b: index.point(b),
    })
}

/// The circle a click on a round edge picks: the chain of real edges through the one nearest the
/// click, followed round its gentlest turns, trimmed to the part that lies on one circle.
pub fn pick_circle(index: &MeshIndex, triangle: u32, hit: V3) -> Result<Feature, String> {
    index.check_triangle(triangle)?;
    const NONE_HERE: &str =
        "No round edge found here. Click on or next to the rim of a hole or a round edge.";
    let (start, _) = index.nearest_feature_edge(triangle, hit).ok_or(NONE_HERE)?;
    let [u, v] = index.edges.ends(start);
    let turn_cos = 60f64.to_radians().cos();
    let walk = |mut from: u32, mut back: u32, mut d: V3| -> (Vec<u32>, bool) {
        let mut chain = Vec::new();
        while chain.len() < 8192 {
            let next = index
                .edges_at(from)
                .iter()
                .copied()
                .filter(|&e| e != back && index.feature[e as usize])
                .filter_map(|e| {
                    let [a, b] = index.edges.ends(e);
                    let to = if a == from { b } else { a };
                    let step = unit(sub(index.point(to), index.point(from)))?;
                    Some((dot(step, d), e, to, step))
                })
                .max_by(|x, y| x.0.total_cmp(&y.0));
            match next {
                Some((c, e, to, step)) if c >= turn_cos => {
                    if e == start {
                        return (chain, true);
                    }
                    chain.push(to);
                    back = e;
                    from = to;
                    d = step;
                }
                _ => break,
            }
        }
        (chain, false)
    };
    let dir = unit(sub(index.point(v), index.point(u))).ok_or(NONE_HERE)?;
    let (forward, closed) = walk(v, start, dir);
    let mut ids: Vec<u32> = vec![u, v];
    ids.extend(forward.iter().copied().filter(|&w| w != u));
    if !closed {
        let (backward, _) = walk(u, start, scale(dir, -1.0));
        let mut before: Vec<u32> = backward.into_iter().rev().collect();
        before.extend(ids);
        ids = before;
    }
    ids.dedup();
    let mut points: Vec<V3> = ids.iter().map(|&w| index.point(w)).collect();
    // Drop the end that strays most until what is left lies on one circle.
    loop {
        let (circle, rms) = fit_circle(&points).ok_or(NONE_HERE)?;
        let Feature::Circle {
            centre,
            normal,
            radius,
        } = circle
        else {
            unreachable!()
        };
        if rms <= CIRCLE_FIT * radius {
            if points.len() < 4 {
                return Err(NONE_HERE.into());
            }
            return Ok(Feature::Circle {
                centre,
                normal,
                radius,
            });
        }
        if points.len() <= 4 {
            return Err(NONE_HERE.into());
        }
        let off = |p: V3| {
            let d = sub(p, centre);
            let h = dot(d, normal);
            (h * h + (length(sub(d, scale(normal, h))) - radius).powi(2)).sqrt()
        };
        if off(points[0]) >= off(points[points.len() - 1]) {
            points.remove(0);
        } else {
            points.pop();
        }
    }
}

/// A flat surface a click picks: the triangles joined to the clicked one whose normals stay within
/// SURFACE_ANGLE of it, as a plane through their centre with their mean normal, and its outline
/// (x, y, z, x, y, z per border segment; empty for an outline too long to draw).
pub fn pick_surface(index: &MeshIndex, triangle: u32) -> Result<(Feature, Vec<f64>), String> {
    index.check_triangle(triangle)?;
    let seed = index.normals[triangle as usize];
    if seed == [0.0; 3] {
        return Err("That triangle has no area; click another one.".into());
    }
    let cos = SURFACE_ANGLE.to_radians().cos();
    let count = index.normals.len();
    let mut inside = vec![false; count];
    let mut queue = vec![triangle];
    inside[triangle as usize] = true;
    let mut area_sum = 0.0;
    let mut normal = [0.0; 3];
    let mut centre = [0.0; 3];
    let mut border: Vec<(u32, u32)> = Vec::new();
    while let Some(t) = queue.pop() {
        let [a, b, c] = index.corners(t).map(|v| index.point(v));
        let n = cross(sub(b, a), sub(c, a));
        let area = length(n) / 2.0;
        area_sum += area;
        normal = add(normal, scale(n, 0.5));
        centre = add(centre, scale(add(add(a, b), c), area / 3.0));
        for k in 0..3u32 {
            let side = t * 3 + k;
            let twin = index.edges.twin(side);
            let joins = twin != NONE && dot(index.normals[twin as usize / 3], seed) >= cos;
            if joins {
                let other = twin / 3;
                if !inside[other as usize] {
                    inside[other as usize] = true;
                    queue.push(other);
                }
            } else {
                let corners = index.corners(t);
                border.push((corners[k as usize], corners[(k as usize + 1) % 3]));
            }
        }
    }
    let normal = unit(normal).ok_or("That surface has no area; click another one.")?;
    let centre = scale(centre, 1.0 / area_sum.max(1e-300));
    let outline = if border.len() <= 20_000 {
        border
            .iter()
            .flat_map(|&(a, b)| [index.point(a), index.point(b)].into_iter().flatten())
            .collect()
    } else {
        Vec::new()
    };
    Ok((
        Feature::Surface {
            point: centre,
            normal,
        },
        outline,
    ))
}

/// A wall's thickness at a click: from the point straight in, against the clicked triangle's
/// outward normal, to where the ray first leaves the material.
pub fn pick_wall(index: &MeshIndex, triangle: u32, hit: V3) -> Result<Feature, String> {
    index.check_triangle(triangle)?;
    let n = index.normals[triangle as usize];
    if n == [0.0; 3] {
        return Err("That triangle has no area; click another one.".into());
    }
    let reach = 2.0 * index.diagonal;
    let end = sub(hit, scale(n, reach));
    let eps = 1e-9 * index.diagonal;
    let mut best: Option<f64> = None;
    index.bvh().segment(hit, end, |t| {
        if t == triangle {
            return;
        }
        let [a, b, c] = index.corners(t).map(|v| index.point(v));
        if let Some(s) = ray_triangle(hit, scale(n, -1.0), a, b, c)
            && s > eps
            && best.is_none_or(|b| s < b)
        {
            best = Some(s);
        }
    });
    let s = best.ok_or("No wall found behind that point: the part is open there.")?;
    Ok(Feature::Wall {
        from: hit,
        to: sub(hit, scale(n, s)),
    })
}

/// Möller–Trumbore: the distance along unit `d` from `o` to the triangle, if the ray meets it.
pub(crate) fn ray_triangle(o: V3, d: V3, a: V3, b: V3, c: V3) -> Option<f64> {
    let (e1, e2) = (sub(b, a), sub(c, a));
    let p = cross(d, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-300 {
        return None;
    }
    let inv = 1.0 / det;
    let s = sub(o, a);
    let u = dot(s, p) * inv;
    if !(-1e-12..=1.0 + 1e-12).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = dot(d, q) * inv;
    if v < -1e-12 || u + v > 1.0 + 1e-12 {
        return None;
    }
    let t = dot(e2, q) * inv;
    (t > 0.0).then_some(t)
}

/// The feature a click picks, in the clicked part's frame; with a surface's outline.
pub fn pick(
    index: &MeshIndex,
    kind: PickKind,
    click: &Click,
) -> Result<(Feature, Vec<f64>), String> {
    let one = |f: Feature| (f, Vec::new());
    match kind {
        PickKind::Point => snap_point(index, click).map(|p| one(Feature::Point { p })),
        PickKind::Edge => pick_edge(index, click.triangle, click.hit).map(one),
        PickKind::Circle => pick_circle(index, click.triangle, click.hit).map(one),
        PickKind::Surface => pick_surface(index, click.triangle),
        PickKind::Wall => pick_wall(index, click.triangle, click.hit).map(one),
    }
}

// ---------------------------------------------------------------------------------------------
// Measuring (world space)

/// What a measurement works out. Names match `MeasureOp` in src/measure/types.ts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Op {
    /// Between two features; circles and spheres count as their centres, except between two
    /// circles, which measure from rim to rim.
    Distance,
    /// Between two circles' centres.
    CentreDistance,
    /// Between two edges, an edge and a surface, or two surfaces.
    Angle,
    /// Between an edge or a surface and the YZ, XZ or XY plane.
    AngleToYz,
    AngleToXz,
    AngleToXy,
    /// Of a circle or a sphere.
    Radius,
    /// Of a point, a surface, a circle's or a sphere's centre.
    Position,
    /// Of a wall.
    Thickness,
    /// At the middle one of three points.
    Angle3,
}

/// A measurement's result in world space. Field names match `MeasureResult` in src/measure/types.ts.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Evaluation {
    /// "distance", "angle", "radius", "position" or "thickness".
    pub kind: &'static str,
    /// mm, or degrees for an angle; for a position, its distance from the origin.
    pub value: f64,
    /// The distance along X, Y and Z (positive), or a position's coordinates.
    pub components: Option<V3>,
    /// The dimension line: from, to.
    pub line: Option<[V3; 2]>,
    /// An angle's corner and the two directions it is between (unit).
    pub apex: Option<V3>,
    pub arms: Option<[V3; 2]>,
    /// A surface's normal, for its position.
    pub normal: Option<V3>,
    /// Where the label goes.
    pub anchor: V3,
    /// The features in world space, for drawing.
    pub features: Vec<Feature>,
    /// Said under the value: "The edges are not parallel; this is the shortest distance between them."
    pub note: Option<String>,
}

fn anchor_of(f: &Feature) -> V3 {
    match *f {
        Feature::Point { p } => p,
        Feature::Edge { a, b } => scale(add(a, b), 0.5),
        Feature::Circle { centre, .. } | Feature::Sphere { centre, .. } => centre,
        Feature::Surface { point, .. } => point,
        Feature::Wall { from, to } => scale(add(from, to), 0.5),
    }
}

/// A feature that measures as a point (a circle's or sphere's centre), if it does.
fn as_point(f: &Feature) -> Option<V3> {
    match *f {
        Feature::Point { p } => Some(p),
        Feature::Circle { centre, .. } | Feature::Sphere { centre, .. } => Some(centre),
        _ => None,
    }
}

/// The nearest point to `p` on the line through `a` and `b`.
fn on_line(p: V3, a: V3, b: V3) -> V3 {
    let d = sub(b, a);
    let dd = dot(d, d);
    if dd == 0.0 {
        return a;
    }
    add(a, scale(d, dot(sub(p, a), d) / dd))
}

fn on_plane(p: V3, point: V3, normal: V3) -> V3 {
    sub(p, scale(normal, dot(sub(p, point), normal)))
}

/// The nearest point to `x` on a circle.
fn on_circle(x: V3, centre: V3, normal: V3, radius: f64) -> V3 {
    let d = sub(x, centre);
    let inplane = sub(d, scale(normal, dot(d, normal)));
    let dir = unit(inplane).unwrap_or_else(|| basis(normal).0);
    add(centre, scale(dir, radius))
}

/// The nearest points of two circles, by a search round the first one refined by golden sections.
fn circle_to_circle(c1: (V3, V3, f64), c2: (V3, V3, f64)) -> [V3; 2] {
    let (u, v) = basis(c1.1);
    let at = |t: f64| {
        add(
            c1.0,
            add(scale(u, c1.2 * t.cos()), scale(v, c1.2 * t.sin())),
        )
    };
    let gap = |t: f64| {
        let p = at(t);
        distance(p, on_circle(p, c2.0, c2.1, c2.2))
    };
    let steps = 720;
    let step = 2.0 * PI / steps as f64;
    let mut best = (0..steps)
        .map(|i| i as f64 * step)
        .min_by(|a, b| gap(*a).total_cmp(&gap(*b)))
        .unwrap();
    let (mut lo, mut hi) = (best - step, best + step);
    let g = (5f64.sqrt() - 1.0) / 2.0;
    for _ in 0..80 {
        let (x1, x2) = (hi - g * (hi - lo), lo + g * (hi - lo));
        if gap(x1) < gap(x2) {
            hi = x2;
        } else {
            lo = x1;
        }
    }
    best = (lo + hi) / 2.0;
    let p = at(best);
    [p, on_circle(p, c2.0, c2.1, c2.2)]
}

/// The nearest points of two features, and a note when the answer needs one.
fn closest(a: &Feature, b: &Feature) -> Result<([V3; 2], Option<String>), String> {
    use Feature::*;
    if let (
        Circle {
            centre: c1,
            normal: n1,
            radius: r1,
        },
        Circle {
            centre: c2,
            normal: n2,
            radius: r2,
        },
    ) = (a, b)
    {
        return Ok((circle_to_circle((*c1, *n1, *r1), (*c2, *n2, *r2)), None));
    }
    if let (Some(p), Some(q)) = (as_point(a), as_point(b)) {
        return Ok(([p, q], None));
    }
    let swap = |r: Result<([V3; 2], Option<String>), String>| r.map(|([x, y], n)| ([y, x], n));
    match (a, b) {
        (_, Edge { a: e1, b: e2 }) if as_point(a).is_some() => {
            let p = as_point(a).unwrap();
            Ok(([p, on_line(p, *e1, *e2)], None))
        }
        (Edge { .. }, _) if as_point(b).is_some() => swap(closest(b, a)),
        (_, Surface { point, normal }) if as_point(a).is_some() => {
            let p = as_point(a).unwrap();
            Ok(([p, on_plane(p, *point, *normal)], None))
        }
        (Surface { .. }, _) if as_point(b).is_some() => swap(closest(b, a)),
        (Edge { a: a1, b: b1 }, Edge { a: a2, b: b2 }) => {
            let d1 = unit(sub(*b1, *a1)).ok_or("An edge has no length.")?;
            let d2 = unit(sub(*b2, *a2)).ok_or("An edge has no length.")?;
            let n = cross(d1, d2);
            if length(n) <= PARALLEL_ANGLE.to_radians().sin() {
                return Ok(([*a1, on_line(*a1, *a2, *b2)], None));
            }
            // The shortest segment between two skew lines.
            let w = sub(*a1, *a2);
            let (b_, d_, e_) = (dot(d1, d2), dot(d1, w), dot(d2, w));
            let denom = 1.0 - b_ * b_;
            let s = (b_ * e_ - d_) / denom;
            let t = (e_ - b_ * d_) / denom;
            Ok((
                [add(*a1, scale(d1, s)), add(*a2, scale(d2, t))],
                Some("The edges are not parallel; this is the shortest distance between the lines through them.".into()),
            ))
        }
        _ => Err("These two cannot be measured against each other.".into()),
    }
}

fn direction(f: &Feature) -> Option<V3> {
    match *f {
        Feature::Edge { a, b } => unit(sub(b, a)),
        _ => None,
    }
}

fn degrees(cos: f64) -> f64 {
    cos.clamp(-1.0, 1.0).acos().to_degrees()
}

fn result(kind: &'static str, value: f64, anchor: V3, features: Vec<Feature>) -> Evaluation {
    Evaluation {
        kind,
        value,
        components: None,
        line: None,
        apex: None,
        arms: None,
        normal: None,
        anchor,
        features,
        note: None,
    }
}

/// What `op` measures on `features` (part-local), taken to world space by `matrix`.
pub fn evaluate(op: Op, features: &[Feature], matrix: &Matrix) -> Result<Evaluation, String> {
    let world: Vec<Feature> = features
        .iter()
        .map(|f| transform(f, matrix))
        .collect::<Result<_, _>>()?;
    let wrong = || -> String { "The measurement's features do not fit its kind.".into() };
    match (op, world.as_slice()) {
        (Op::Distance | Op::CentreDistance, [a, b]) => {
            let ([p, q], note) = if op == Op::CentreDistance {
                (
                    [
                        as_point(a).ok_or_else(wrong)?,
                        as_point(b).ok_or_else(wrong)?,
                    ],
                    None,
                )
            } else {
                closest(a, b)?
            };
            let d = sub(q, p);
            let mut out = result("distance", length(d), scale(add(p, q), 0.5), world.clone());
            out.components = Some(d.map(f64::abs));
            out.line = Some([p, q]);
            out.note = note;
            Ok(out)
        }
        (Op::Angle, [a, b]) => {
            use Feature::*;
            let value = match (a, b) {
                (Edge { .. }, Edge { .. }) => degrees(
                    dot(
                        direction(a).ok_or_else(wrong)?,
                        direction(b).ok_or_else(wrong)?,
                    )
                    .abs(),
                ),
                (Edge { .. }, Surface { normal, .. }) | (Surface { normal, .. }, Edge { .. }) => {
                    let d = direction(if matches!(a, Edge { .. }) { a } else { b })
                        .ok_or_else(wrong)?;
                    90.0 - degrees(dot(d, *normal).abs())
                }
                (Surface { normal: n1, .. }, Surface { normal: n2, .. }) => degrees(dot(*n1, *n2)),
                _ => return Err(wrong()),
            };
            Ok(result(
                "angle",
                value,
                scale(add(anchor_of(a), anchor_of(b)), 0.5),
                world.clone(),
            ))
        }
        (Op::AngleToYz | Op::AngleToXz | Op::AngleToXy, [f]) => {
            let axis = match op {
                Op::AngleToYz => [1.0, 0.0, 0.0],
                Op::AngleToXz => [0.0, 1.0, 0.0],
                _ => [0.0, 0.0, 1.0],
            };
            let value = match f {
                Feature::Edge { .. } => {
                    90.0 - degrees(dot(direction(f).ok_or_else(wrong)?, axis).abs())
                }
                Feature::Surface { normal, .. } => degrees(dot(*normal, axis).abs()),
                _ => return Err(wrong()),
            };
            Ok(result("angle", value, anchor_of(f), world.clone()))
        }
        (Op::Radius, [f @ (Feature::Circle { radius, .. } | Feature::Sphere { radius, .. })]) => {
            let mut out = result("radius", *radius, anchor_of(f), world.clone());
            if let Feature::Circle {
                centre,
                normal,
                radius,
            } = *f
            {
                out.line = Some([centre, add(centre, scale(basis(normal).0, radius))]);
                out.normal = Some(normal);
            } else if let Feature::Sphere { centre, radius } = *f {
                out.line = Some([centre, add(centre, [radius, 0.0, 0.0])]);
            }
            Ok(out)
        }
        (Op::Position, [f]) => {
            let (p, normal) = match *f {
                Feature::Surface { point, normal } => (point, Some(normal)),
                _ => (as_point(f).ok_or_else(wrong)?, None),
            };
            let mut out = result("position", length(p), p, world.clone());
            out.components = Some(p);
            out.normal = normal;
            Ok(out)
        }
        (Op::Thickness, [f @ Feature::Wall { from, to }]) => {
            let mut out = result(
                "thickness",
                distance(*from, *to),
                anchor_of(f),
                world.clone(),
            );
            out.line = Some([*from, *to]);
            out.components = Some(sub(*to, *from).map(f64::abs));
            Ok(out)
        }
        (Op::Angle3, [a, b, c]) => {
            let (p, q, r) = (
                as_point(a).ok_or_else(wrong)?,
                as_point(b).ok_or_else(wrong)?,
                as_point(c).ok_or_else(wrong)?,
            );
            let u = unit(sub(p, q)).ok_or("The first two points are the same.")?;
            let w = unit(sub(r, q)).ok_or("The last two points are the same.")?;
            let mut out = result("angle", degrees(dot(u, w)), q, world.clone());
            out.apex = Some(q);
            out.arms = Some([u, w]);
            Ok(out)
        }
        _ => Err(wrong()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library;
    use crate::mesh::{IDENTITY, make_box};
    use serde_json::json;

    fn index(mesh: Mesh) -> MeshIndex {
        MeshIndex::new(mesh)
    }

    /// The triangle whose interior holds `p` (exactly on its plane) and faces along `normal`.
    fn triangle_at(index: &MeshIndex, p: V3, normal: V3) -> u32 {
        (0..index.normals.len() as u32)
            .find(|&t| {
                let [a, b, c] = index.corners(t).map(|v| index.point(v));
                dot(index.normals[t as usize], normal) > 0.999 && {
                    let n = cross(sub(b, a), sub(c, a));
                    [(a, b), (b, c), (c, a)]
                        .iter()
                        .all(|&(x, y)| dot(cross(sub(y, x), sub(p, x)), n) >= -1e-9)
                        && dot(sub(p, a), index.normals[t as usize]).abs() < 1e-6
                }
            })
            .unwrap_or_else(|| panic!("no triangle at {p:?} facing {normal:?}"))
    }

    fn click(index: &MeshIndex, hit: V3, normal: V3, snap: Snap) -> Click<'static> {
        Click {
            triangle: triangle_at(index, hit, normal),
            hit,
            snap,
            tolerance: 1.0,
            clip: &[],
        }
    }

    fn near(a: V3, b: V3, tol: f64) -> bool {
        distance(a, b) <= tol
    }

    fn eval(op: Op, features: &[Feature]) -> Evaluation {
        evaluate(op, features, &IDENTITY).unwrap()
    }

    const UP: V3 = [0.0, 0.0, 1.0];

    #[test]
    fn points_snap_to_corners_edges_or_stay_free() {
        let cube = index(make_box([0.0; 3], [20.0; 3]));
        let at = |hit: V3, snap| snap_point(&cube, &click(&cube, hit, UP, snap)).unwrap();
        assert_eq!(at([5.0, 6.0, 20.0], Snap::Free), [5.0, 6.0, 20.0]);
        assert_eq!(at([5.0, 6.0, 20.0], Snap::Node), [0.0, 0.0, 20.0]);
        assert_eq!(at([5.0, 0.5, 20.0], Snap::Edge), [5.0, 0.0, 20.0]);
        // Automatic: a corner within the tolerance, else an edge within it, else the point itself.
        assert_eq!(at([0.5, 0.4, 20.0], Snap::Auto), [0.0, 0.0, 20.0]);
        assert_eq!(at([5.0, 0.5, 20.0], Snap::Auto), [5.0, 0.0, 20.0]);
        assert_eq!(at([5.0, 6.0, 20.0], Snap::Auto), [5.0, 6.0, 20.0]);
    }

    #[test]
    fn points_snap_to_where_a_clip_plane_cuts_the_part() {
        let cube = index(make_box([0.0; 3], [20.0; 3]));
        // Keeps x <= 7: normal −X, constant 7.
        let plane = [ClipPlane {
            normal: [-1.0, 0.0, 0.0],
            constant: 7.0,
        }];
        let mut c = click(&cube, [6.5, 9.0, 20.0], UP, Snap::ClipLine);
        c.clip = &plane;
        assert!(near(snap_point(&cube, &c).unwrap(), [7.0, 9.0, 20.0], 1e-9));
        c.snap = Snap::ClipCorner;
        assert!(
            near(snap_point(&cube, &c).unwrap(), [7.0, 20.0, 20.0], 1e-9)
                || near(snap_point(&cube, &c).unwrap(), [7.0, 0.0, 20.0], 1e-9)
        );
        c.clip = &[];
        assert!(
            snap_point(&cube, &c)
                .unwrap_err()
                .starts_with("Turn on a clip plane")
        );
    }

    #[test]
    fn cube_edges_measure_their_length_distance_and_angles() {
        let cube = index(make_box([0.0; 3], [20.0; 3]));
        let edge = |hit: V3, n: V3| pick_edge(&cube, triangle_at(&cube, hit, n), hit).unwrap();
        let front_top = edge([10.0, 0.3, 20.0], UP);
        let Feature::Edge { a, b } = front_top else {
            panic!()
        };
        assert_eq!(distance(a, b), 20.0);
        let back_top = edge([10.0, 19.7, 20.0], UP);
        let front_bottom = edge([10.0, 0.0, 0.3], [0.0, -1.0, 0.0]);
        let upright = edge([0.3, 0.0, 10.0], [0.0, -1.0, 0.0]);
        let d = eval(Op::Distance, &[front_top.clone(), back_top.clone()]);
        assert!((d.value - 20.0).abs() < 1e-12 && d.note.is_none());
        assert!(near(d.components.unwrap(), [0.0, 20.0, 0.0], 1e-12));
        assert!(
            (eval(Op::Distance, &[front_top.clone(), front_bottom]).value - 20.0).abs() < 1e-12
        );
        assert!((eval(Op::Angle, &[front_top.clone(), upright.clone()]).value - 90.0).abs() < 1e-9);
        assert!(
            eval(Op::AngleToXy, std::slice::from_ref(&front_top))
                .value
                .abs()
                < 1e-9
        );
        assert!((eval(Op::AngleToXy, std::slice::from_ref(&upright)).value - 90.0).abs() < 1e-9);
        assert!((eval(Op::AngleToYz, std::slice::from_ref(&front_top)).value - 90.0).abs() < 1e-9);
        // Skew edges: the shortest distance between the lines, with a note.
        let side_top = edge([19.7, 10.0, 20.0], UP);
        let skew = eval(Op::Distance, &[side_top, upright.clone()]);
        assert!(skew.note.is_some());
        assert!((skew.value - 20.0).abs() < 1e-12);
        assert!(near(skew.line.unwrap()[1], [0.0, 0.0, 20.0], 1e-12));
        // Two edges that meet at a corner: no distance between them.
        let left_bottom = front_bottom_left(&cube);
        assert!((eval(Op::Distance, &[upright.clone(), left_bottom]).value - 0.0).abs() < 1e-12);
        let p = eval(
            Op::Distance,
            &[
                Feature::Point {
                    p: [10.0, 10.0, 25.0],
                },
                front_top,
            ],
        );
        assert!((p.value - (100.0f64 + 25.0).sqrt()).abs() < 1e-12);
    }

    fn front_bottom_left(cube: &MeshIndex) -> Feature {
        pick_edge(
            cube,
            triangle_at(cube, [0.0, 10.0, 0.3], [-1.0, 0.0, 0.0]),
            [0.0, 10.0, 0.3],
        )
        .unwrap()
    }

    #[test]
    fn surfaces_are_planes_with_angles_and_distances() {
        let cube = index(make_box([0.0; 3], [20.0; 3]));
        let face = |hit: V3, n: V3| pick_surface(&cube, triangle_at(&cube, hit, n)).unwrap();
        let (top, outline) = face([3.0, 4.0, 20.0], UP);
        let Feature::Surface { point, normal } = top else {
            panic!()
        };
        assert!(near(point, [10.0, 10.0, 20.0], 1e-12) && normal == UP);
        assert_eq!(outline.len(), 4 * 6);
        let (bottom, _) = face([3.0, 4.0, 0.0], [0.0, 0.0, -1.0]);
        let (front, _) = face([3.0, 0.0, 4.0], [0.0, -1.0, 0.0]);
        assert!((eval(Op::Angle, &[top.clone(), front.clone()]).value - 90.0).abs() < 1e-9);
        assert!((eval(Op::Angle, &[top.clone(), bottom]).value - 180.0).abs() < 1e-9);
        assert!(eval(Op::AngleToXy, std::slice::from_ref(&top)).value.abs() < 1e-9);
        assert!((eval(Op::AngleToXz, &[front]).value).abs() < 1e-9);
        let d = eval(
            Op::Distance,
            &[Feature::Point { p: [1.0, 2.0, 3.0] }, top.clone()],
        );
        assert!((d.value - 17.0).abs() < 1e-12);
        let pos = eval(Op::Position, &[top]);
        assert!(near(pos.components.unwrap(), [10.0, 10.0, 20.0], 1e-12) && pos.normal == Some(UP));
    }

    fn cylinder(radius: f64, height: f64, segments: f64) -> MeshIndex {
        let p = json!({"radius": radius, "height": height, "segments": segments});
        index(library::generate("cylinder", p.as_object().unwrap()).unwrap())
    }

    #[test]
    fn a_cylinder_rim_is_detected_as_its_circle() {
        let cyl = cylinder(10.0, 20.0, 64.0);
        // Near the top rim, on the top face.
        let hit = [9.6, 0.4, 20.0];
        let top = pick_circle(&cyl, triangle_at(&cyl, hit, UP), hit).unwrap();
        let Feature::Circle {
            centre,
            normal,
            radius,
        } = top
        else {
            panic!()
        };
        assert!(near(centre, [0.0, 0.0, 20.0], 1e-4), "{centre:?}");
        assert!((radius - 10.0).abs() < 1e-4, "{radius}");
        assert!(dot(normal, UP).abs() > 1.0 - 1e-9);
        let hit = [9.6, 0.4, 0.0];
        let bottom = pick_circle(&cyl, triangle_at(&cyl, hit, [0.0, 0.0, -1.0]), hit).unwrap();
        assert!(
            (eval(Op::CentreDistance, &[top.clone(), bottom.clone()]).value - 20.0).abs() < 1e-4
        );
        assert!((eval(Op::Distance, &[top.clone(), bottom.clone()]).value - 20.0).abs() < 1e-4);
        assert!((eval(Op::Radius, std::slice::from_ref(&top)).value - 10.0).abs() < 1e-4);
        // A point on the axis to the rim's circle: its centre.
        assert!(
            (eval(Op::Distance, &[Feature::Point { p: [0.0, 0.0, 5.0] }, top]).value - 15.0).abs()
                < 1e-4
        );
        // A box has no round edge.
        let cube = index(make_box([0.0; 3], [20.0; 3]));
        let hit = [10.0, 0.4, 20.0];
        assert!(pick_circle(&cube, triangle_at(&cube, hit, UP), hit).is_err());
    }

    #[test]
    fn three_points_make_a_circle_and_four_a_sphere() {
        let Feature::Circle {
            centre,
            radius,
            normal,
        } = circle_through([[10.0, 0.0, 5.0], [0.0, 10.0, 5.0], [-10.0, 0.0, 5.0]]).unwrap()
        else {
            panic!()
        };
        assert!(
            near(centre, [0.0, 0.0, 5.0], 1e-12)
                && (radius - 10.0).abs() < 1e-12
                && near(normal, UP, 1e-12)
        );
        assert!(circle_through([[0.0; 3], [1.0, 1.0, 1.0], [2.0, 2.0, 2.0]]).is_err());
        // Four vertices of a sphere mesh lie on the sphere.
        let p = json!({"radius": 7.5});
        let sphere = library::generate("sphere", p.as_object().unwrap()).unwrap();
        let pts = [3u32, 400, 900, 1500].map(|v| sphere.point(v));
        let Feature::Sphere { centre, radius } = sphere_through(pts).unwrap() else {
            panic!()
        };
        assert!(near(centre, [0.0, 0.0, 7.5], 1e-5), "{centre:?}");
        assert!((radius - 7.5).abs() < 1e-5);
        assert!(
            sphere_through([[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]]).is_err()
        );
        let s = Feature::Sphere { centre, radius };
        assert!((eval(Op::Radius, std::slice::from_ref(&s)).value - 7.5).abs() < 1e-5);
        assert!(
            (eval(
                Op::Distance,
                &[
                    s.clone(),
                    Feature::Point {
                        p: [0.0, 0.0, 17.5]
                    }
                ]
            )
            .value
                - 10.0)
                .abs()
                < 1e-5
        );
        assert!((eval(Op::Distance, &[s.clone(), s]).value).abs() < 1e-12);
    }

    #[test]
    fn a_plate_is_as_thick_as_it_was_made() {
        let plate = index(make_box([0.0; 3], [50.0, 40.0, 3.0]));
        let hit = [20.0, 13.0, 3.0];
        let wall = pick_wall(&plate, triangle_at(&plate, hit, UP), hit).unwrap();
        assert_eq!(
            wall,
            Feature::Wall {
                from: hit,
                to: [20.0, 13.0, 0.0]
            }
        );
        assert_eq!(eval(Op::Thickness, &[wall]).value, 3.0);
        // From a side face, straight across.
        let hit = [50.0, 13.0, 1.0];
        let across = pick_wall(&plate, triangle_at(&plate, hit, [1.0, 0.0, 0.0]), hit).unwrap();
        assert_eq!(eval(Op::Thickness, &[across]).value, 50.0);
        // A box without its floor: nothing behind its top.
        let mut open = make_box([0.0; 3], [10.0; 3]);
        open.indices.drain(0..6);
        let open = index(open);
        let hit = [5.0, 5.0, 10.0];
        assert!(pick_wall(&open, triangle_at(&open, hit, UP), hit).is_err());
    }

    #[test]
    fn three_points_give_the_angle_at_the_middle_one() {
        let p = |x, y| Feature::Point { p: [x, y, 0.0] };
        let a = eval(Op::Angle3, &[p(10.0, 0.0), p(0.0, 0.0), p(0.0, 5.0)]);
        assert!((a.value - 90.0).abs() < 1e-12);
        assert_eq!(a.apex, Some([0.0; 3]));
        assert!(
            (eval(Op::Angle3, &[p(1.0, 1.0), p(0.0, 0.0), p(-1.0, 0.0)]).value - 135.0).abs()
                < 1e-12
        );
        assert!(
            evaluate(
                Op::Angle3,
                &[p(0.0, 0.0), p(0.0, 0.0), p(1.0, 0.0)],
                &IDENTITY
            )
            .is_err()
        );
    }

    #[test]
    fn measurements_follow_the_part_matrix() {
        // Rotate 90° about Z, scale 2, move.
        let m: Matrix = [
            0.0, 2.0, 0.0, 0.0, -2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 5.0, 6.0, 7.0, 1.0,
        ];
        let edge = Feature::Edge {
            a: [0.0; 3],
            b: [10.0, 0.0, 0.0],
        };
        let circle = Feature::Circle {
            centre: [0.0, 0.0, 1.0],
            normal: [1.0, 0.0, 0.0],
            radius: 3.0,
        };
        let d = evaluate(
            Op::Distance,
            &[Feature::Point { p: [0.0, 5.0, 0.0] }, edge.clone()],
            &m,
        )
        .unwrap();
        assert!((d.value - 10.0).abs() < 1e-12);
        assert!((evaluate(Op::AngleToYz, &[edge], &m).unwrap().value).abs() < 1e-9);
        let r = evaluate(Op::Radius, &[circle], &m).unwrap();
        assert!((r.value - 6.0).abs() < 1e-12);
        let Feature::Circle { centre, normal, .. } = r.features[0] else {
            panic!()
        };
        assert!(near(centre, [5.0, 6.0, 9.0], 1e-12) && near(normal, [0.0, 1.0, 0.0], 1e-12));
        let pos = evaluate(
            Op::Position,
            &[Feature::Surface {
                point: [1.0, 0.0, 0.0],
                normal: [1.0, 0.0, 0.0],
            }],
            &m,
        )
        .unwrap();
        assert!(
            near(pos.components.unwrap(), [5.0, 8.0, 7.0], 1e-12)
                && near(pos.normal.unwrap(), [0.0, 1.0, 0.0], 1e-12)
        );
        // A feature moved into another part's frame and back is unchanged.
        let inv = invert(&m).unwrap();
        let f = Feature::Sphere {
            centre: [1.0, 2.0, 3.0],
            radius: 4.0,
        };
        let back = transform(&transform(&f, &m).unwrap(), &inv).unwrap();
        let Feature::Sphere { centre, radius } = back else {
            panic!()
        };
        assert!(near(centre, [1.0, 2.0, 3.0], 1e-12) && (radius - 4.0).abs() < 1e-12);
        assert!(evaluate(Op::Radius, &[Feature::Point { p: [0.0; 3] }], &m).is_err());
    }
}
