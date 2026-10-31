//! Standard analysis: position, size, volume and area; the mesh counts; the damage counts; closed
//! and orientable; and the spread of edges per point, triangles per edge, triangle quality and
//! edge length. The counts are the Repair Status tab's (repair::status) and volume and area are
//! worked out exactly as the Information panel does (src/info.ts), so the numbers agree.

use serde::Serialize;

use super::{Spread, SpreadOf};
use crate::arrange::{V3, len, sub};
use crate::mesh::{Matrix, Mesh, linear_determinant, transform_point, volume_and_area};
use crate::repair::status;
use crate::repair::topology::{Edges, orientation_patches};
use crate::repair::{Control, RepairResult};

/// `StandardResult` in src/analysis/types.ts. Lengths in mm, area in mm², volume in mm³.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Standard {
    pub min: V3,
    pub max: V3,
    pub size: V3,
    /// |det A × local signed volume|, as the Information panel shows it.
    pub volume: f64,
    /// False when the mesh has open edges: the Information panel then shows no volume.
    pub volume_valid: bool,
    pub area: f64,
    pub points: u64,
    pub triangles: u64,
    pub edges: u64,
    pub shells: u64,
    pub holes: u64,
    pub boundary_edges: u64,
    pub boundary_length: f64,
    pub flipped_triangles: u64,
    /// Edges with three or more triangles (the Status tab's non-manifold edges).
    pub bad_edges: u64,
    pub closed: bool,
    /// Every pair of triangles across an edge can be made to agree (no Möbius-like patch).
    pub orientable: bool,
    pub edges_per_point: Spread,
    pub triangles_per_edge: Spread,
    /// 4√3 · area / (sum of squared edge lengths): 1 for an equilateral triangle, 0 for a flat one.
    pub triangle_quality: Spread,
    pub edge_length: Spread,
}

/// `similarityScale2` in src/info.ts: s² when the linear part is s × a rotation (maybe mirrored).
fn similarity_scale2(m: &Matrix) -> Option<f64> {
    const TOL: f64 = 1e-7;
    let l0 = m[0] * m[0] + m[1] * m[1] + m[2] * m[2];
    let l1 = m[4] * m[4] + m[5] * m[5] + m[6] * m[6];
    let l2 = m[8] * m[8] + m[9] * m[9] + m[10] * m[10];
    let s2 = (l0 + l1 + l2) / 3.0;
    if s2 == 0.0 {
        return Some(0.0);
    }
    let tol = TOL * s2;
    let d01 = m[0] * m[4] + m[1] * m[5] + m[2] * m[6];
    let d02 = m[0] * m[8] + m[1] * m[9] + m[2] * m[10];
    let d12 = m[4] * m[8] + m[5] * m[9] + m[6] * m[10];
    let ok = (l0 - s2).abs() <= tol
        && (l1 - s2).abs() <= tol
        && (l2 - s2).abs() <= tol
        && d01.abs() <= tol
        && d02.abs() <= tol
        && d12.abs() <= tol;
    ok.then_some(s2)
}

/// `transformedArea` in src/info.ts: the exact world area without moving a vertex.
fn transformed_area(mesh: &Mesh, m: &Matrix) -> f64 {
    let p = &mesh.positions;
    let (a0x, a0y, a0z) = (m[0], m[1], m[2]);
    let (a1x, a1y, a1z) = (m[4], m[5], m[6]);
    let (a2x, a2y, a2z) = (m[8], m[9], m[10]);
    let (k0x, k0y, k0z) = (
        a1y * a2z - a1z * a2y,
        a1z * a2x - a1x * a2z,
        a1x * a2y - a1y * a2x,
    );
    let (k1x, k1y, k1z) = (
        a2y * a0z - a2z * a0y,
        a2z * a0x - a2x * a0z,
        a2x * a0y - a2y * a0x,
    );
    let (k2x, k2y, k2z) = (
        a0y * a1z - a0z * a1y,
        a0z * a1x - a0x * a1z,
        a0x * a1y - a0y * a1x,
    );
    let mut sum = 0.0;
    for t in mesh.indices.as_chunks::<3>().0 {
        let (i0, i1, i2) = (t[0] as usize * 3, t[1] as usize * 3, t[2] as usize * 3);
        let (ox, oy, oz) = (p[i0] as f64, p[i0 + 1] as f64, p[i0 + 2] as f64);
        let (ux, uy, uz) = (
            p[i1] as f64 - ox,
            p[i1 + 1] as f64 - oy,
            p[i1 + 2] as f64 - oz,
        );
        let (vx, vy, vz) = (
            p[i2] as f64 - ox,
            p[i2 + 1] as f64 - oy,
            p[i2 + 2] as f64 - oz,
        );
        let nx = uy * vz - uz * vy;
        let ny = uz * vx - ux * vz;
        let nz = ux * vy - uy * vx;
        let wx = k0x * nx + k1x * ny + k2x * nz;
        let wy = k0y * nx + k1y * ny + k2y * nz;
        let wz = k0z * nx + k1z * ny + k2z * nz;
        sum += (wx * wx + wy * wy + wz * wz).sqrt();
    }
    sum / 2.0
}

/// The world volume and area exactly as the Information panel works them out from the part's
/// local stats and its matrix.
pub fn panel_volume_and_area(mesh: &Mesh, m: &Matrix) -> (f64, f64) {
    let (volume, area) = volume_and_area(mesh);
    let world_area = match similarity_scale2(m) {
        Some(s2) => area * s2,
        None => transformed_area(mesh, m),
    };
    ((linear_determinant(m) * volume).abs(), world_area)
}

/// 4√3 · area / Σ edge², 0 for a triangle with no extent.
pub fn triangle_quality(a: V3, b: V3, c: V3) -> f64 {
    let (ab, bc, ca) = (sub(b, a), sub(c, b), sub(a, c));
    let squares =
        crate::arrange::dot(ab, ab) + crate::arrange::dot(bc, bc) + crate::arrange::dot(ca, ca);
    if squares <= 0.0 {
        return 0.0;
    }
    let area = len(crate::arrange::cross(ab, sub(c, a))) / 2.0;
    (4.0 * 3f64.sqrt() * area / squares).clamp(0.0, 1.0)
}

/// The standard analysis of the part-local `mesh` placed by `m`.
pub fn analyze(mesh: &Mesh, m: &Matrix, control: &mut Control) -> RepairResult<Standard> {
    control.check()?;
    let detail = status::analyze(mesh, &mut control.sub(0.0, 0.6))?;
    let s = detail.status;
    control.check()?;
    let edges = Edges::new(mesh);
    let orientable = s.oriented || orientation_patches(mesh, &edges).conflicts == 0;
    control.progress(0.7);
    control.check()?;
    // Every vertex, as the Information panel's box takes them (src/mesh.ts boundsOf).
    let points: Vec<V3> = mesh
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| transform_point(m, [p[0] as f64, p[1] as f64, p[2] as f64]))
        .collect();
    let (mut min, mut max) = ([0.0; 3], [0.0; 3]);
    if let Some(first) = points.first() {
        (min, max) = (*first, *first);
        for p in &points {
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
    }
    let (volume, area) = panel_volume_and_area(mesh, m);

    let mut per_point = vec![0u32; mesh.vertex_count()];
    let mut edge_length = SpreadOf::default();
    let mut per_edge = SpreadOf::default();
    let mut boundary_length = 0.0;
    for e in 0..edges.len() as u32 {
        let [a, b] = edges.ends(e);
        per_point[a as usize] += 1;
        per_point[b as usize] += 1;
        let l = len(sub(points[a as usize], points[b as usize]));
        edge_length.add(l);
        let degree = edges.degree(e);
        per_edge.add(degree as f64);
        if degree == 1 {
            boundary_length += l;
        }
    }
    control.progress(0.85);
    control.check()?;
    let mut edges_per_point = SpreadOf::default();
    for &n in per_point.iter().filter(|&&n| n > 0) {
        edges_per_point.add(n as f64);
    }
    let mut quality = SpreadOf::default();
    for t in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|i| points[i as usize]);
        quality.add(triangle_quality(a, b, c));
    }
    control.progress(1.0);
    Ok(Standard {
        min,
        max,
        size: sub(max, min),
        volume,
        volume_valid: s.border_edges == 0,
        area,
        points: s.vertices,
        triangles: s.triangles,
        edges: s.edges,
        shells: s.shells,
        holes: s.holes,
        boundary_edges: s.border_edges,
        boundary_length,
        flipped_triangles: s.inverted_triangles,
        bad_edges: s.non_manifold_edges,
        closed: s.closed,
        orientable,
        edges_per_point: edges_per_point.finish(),
        triangles_per_edge: per_edge.finish(),
        triangle_quality: quality.finish(),
        edge_length: edge_length.finish(),
    })
}
