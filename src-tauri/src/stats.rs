//! Platform statistics (Home › Platform statistics): for each part its mesh status, world size,
//! volume, area and counts, and for a set of parts their box and the platform's totals and packing
//! density. Plain functions over meshes, so tests run them without Tauri.
//!
//! The numbers are the Information panel's (src/info.ts), worked out the same way so the two agree
//! to the last digit shown: the box transforms every vertex in f64, the volume is |det A × V| of the
//! signed local volume, and the area is the local area × s² for a rotation with uniform scale s, the
//! area of the transformed triangles otherwise.
//!
//! Packing density follows the reference tool's help: the volume of the parts that lie wholly on the
//! platform as a share of the platform's volume (X × Y × Z), and "with object height" the same share
//! of X × Y × the height of the highest of those parts. Parts that stick out of the platform are left
//! out of the volume, the area and both densities.

use serde::Serialize;

use crate::mesh::{Matrix, Mesh, cross, dot, linear_determinant, sub, transform_point};
use crate::repair::status;
use crate::repair::{Control, RepairResult};

/// A part sticking out of the platform by less than this (mm) still counts as on it: a part dropped
/// on the platform can end a rounding error below zero.
pub const PLATFORM_TOLERANCE: f64 = 1e-3;

/// Relative tolerance for a matrix being rotation × uniform scale; `SIMILARITY_TOL` in src/info.ts.
const SIMILARITY_TOL: f64 = 1e-7;

/// One row of the parts table. Field names match `PartStats` in src/stats/ipc.ts.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartStats {
    pub triangles: u64,
    /// Undirected edges.
    pub edges: u64,
    /// Vertices.
    pub points: u64,
    pub shells: u64,
    /// mm³ in world space; None when the mesh has holes, as the Information panel shows "—".
    pub volume: Option<f64>,
    /// mm² in world space.
    pub area: f64,
    /// World box, None for a part without vertices.
    pub min: Option<[f64; 3]>,
    pub max: Option<[f64; 3]>,
    pub closed: bool,
    pub oriented: bool,
    pub holes: u64,
    pub border_edges: u64,
    pub non_manifold_edges: u64,
    pub inverted_triangles: u64,
    pub degenerate_triangles: u64,
}

/// The Information section for a set of parts. Field names match `StatsSummary` in src/stats/ipc.ts.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// Parts in the set.
    pub parts: u64,
    /// Their box (Part positions), None when none has a vertex.
    pub min: Option<[f64; 3]>,
    pub max: Option<[f64; 3]>,
    /// Parts of the set that lie wholly on the platform: the ones the totals count.
    pub on_platform: u64,
    /// mm³; None when one of the counted parts has holes.
    pub volume: Option<f64>,
    /// mm².
    pub area: f64,
    /// Percent of the platform's volume.
    pub packing_density: Option<f64>,
    /// Percent of X × Y × the height of the highest counted part.
    pub with_object_height: Option<f64>,
    /// Height of the highest counted part above the platform (mm).
    pub top: Option<f64>,
}

/// s² when the linear part of `m` is a rotation (possibly mirrored) times a uniform scale s; None
/// otherwise. Same test and tolerance as `similarityScale2` in src/info.ts.
pub fn similarity_scale2(m: &Matrix) -> Option<f64> {
    let l0 = m[0] * m[0] + m[1] * m[1] + m[2] * m[2];
    let l1 = m[4] * m[4] + m[5] * m[5] + m[6] * m[6];
    let l2 = m[8] * m[8] + m[9] * m[9] + m[10] * m[10];
    let s2 = (l0 + l1 + l2) / 3.0;
    if s2 == 0.0 {
        return Some(0.0);
    }
    let tol = SIMILARITY_TOL * s2;
    let d01 = m[0] * m[4] + m[1] * m[5] + m[2] * m[6];
    let d02 = m[0] * m[8] + m[1] * m[9] + m[2] * m[10];
    let d12 = m[4] * m[8] + m[5] * m[9] + m[6] * m[10];
    let ok = [l0 - s2, l1 - s2, l2 - s2, d01, d02, d12]
        .iter()
        .all(|v| v.abs() <= tol);
    ok.then_some(s2)
}

/// World box of every vertex under `m`, as `boundsOf` in src/mesh.ts computes it.
pub fn world_bounds(mesh: &Mesh, m: &Matrix) -> Option<[[f64; 3]; 2]> {
    let mut out: Option<[[f64; 3]; 2]> = None;
    for p in mesh.positions.as_chunks::<3>().0 {
        let w = transform_point(m, [p[0] as f64, p[1] as f64, p[2] as f64]);
        let b = out.get_or_insert([w, w]);
        for k in 0..3 {
            b[0][k] = b[0][k].min(w[k]);
            b[1][k] = b[1][k].max(w[k]);
        }
    }
    out
}

/// Area of the triangles moved by `m` (mm²), for matrices that are not a similarity.
fn transformed_area(mesh: &Mesh, m: &Matrix) -> f64 {
    let mut sum = 0.0;
    for t in mesh.indices.as_chunks::<3>().0 {
        let a = transform_point(m, mesh.point(t[0]));
        let b = transform_point(m, mesh.point(t[1]));
        let c = transform_point(m, mesh.point(t[2]));
        let n = cross(sub(b, a), sub(c, a));
        sum += dot(n, n).sqrt();
    }
    sum / 2.0
}

/// The parts-table row of one part. Cancel stops it between its steps.
pub fn part_stats(mesh: &Mesh, m: &Matrix, control: &mut Control) -> RepairResult<PartStats> {
    let detail = status::analyze(mesh, &mut control.sub(0.0, 0.8))?;
    let s = detail.status;
    control.check()?;
    let (local_volume, local_area) = crate::mesh::volume_and_area(mesh);
    let area = match similarity_scale2(m) {
        Some(s2) => local_area * s2,
        None => transformed_area(mesh, m),
    };
    control.progress(0.9);
    control.check()?;
    let bounds = world_bounds(mesh, m);
    control.progress(1.0);
    Ok(PartStats {
        triangles: s.triangles,
        edges: s.edges,
        points: s.vertices,
        shells: s.shells,
        volume: (s.border_edges == 0).then(|| (linear_determinant(m) * local_volume).abs()),
        area,
        min: bounds.map(|b| b[0]),
        max: bounds.map(|b| b[1]),
        closed: s.closed,
        oriented: s.oriented,
        holes: s.holes,
        border_edges: s.border_edges,
        non_manifold_edges: s.non_manifold_edges,
        inverted_triangles: s.inverted_triangles,
        degenerate_triangles: s.degenerate_triangles,
    })
}

/// True when the part's box lies within the platform, 0..size on each axis.
pub fn on_platform(part: &PartStats, platform: [f64; 3]) -> bool {
    let (Some(min), Some(max)) = (part.min, part.max) else {
        return false;
    };
    (0..3).all(|k| min[k] >= -PLATFORM_TOLERANCE && max[k] <= platform[k] + PLATFORM_TOLERANCE)
}

/// The Information section for the parts at `which` (indices into `parts`).
pub fn summary(parts: &[PartStats], which: &[usize], platform: [f64; 3]) -> Summary {
    let chosen: Vec<&PartStats> = which.iter().filter_map(|&i| parts.get(i)).collect();
    let mut min: Option<[f64; 3]> = None;
    let mut max: Option<[f64; 3]> = None;
    for p in &chosen {
        if let (Some(lo), Some(hi)) = (p.min, p.max) {
            let a = min.get_or_insert(lo);
            let b = max.get_or_insert(hi);
            for k in 0..3 {
                a[k] = a[k].min(lo[k]);
                b[k] = b[k].max(hi[k]);
            }
        }
    }
    let counted: Vec<&&PartStats> = chosen.iter().filter(|p| on_platform(p, platform)).collect();
    // A float sum of nothing is -0.0; adding 0.0 turns it into 0.
    let volume = counted
        .iter()
        .map(|p| p.volume)
        .sum::<Option<f64>>()
        .map(|v| v + 0.0);
    let area = counted.iter().map(|p| p.area).sum::<f64>() + 0.0;
    let top = counted
        .iter()
        .filter_map(|p| p.max.map(|m| m[2]))
        .fold(None, |acc: Option<f64>, z| {
            Some(acc.map_or(z, |a| a.max(z)))
        });
    let [x, y, z] = platform;
    let percent = |v: f64, height: f64| (height > 0.0).then(|| 100.0 * v / (x * y * height));
    Summary {
        parts: chosen.len() as u64,
        min,
        max,
        on_platform: counted.len() as u64,
        volume,
        area,
        packing_density: volume.and_then(|v| percent(v, z)),
        with_object_height: volume.and_then(|v| top.and_then(|t| percent(v, t))),
        top,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{IDENTITY, make_box};
    use crate::repair::silent;

    fn translate(x: f64, y: f64, z: f64) -> Matrix {
        let mut m = IDENTITY;
        m[12] = x;
        m[13] = y;
        m[14] = z;
        m
    }

    fn stats_of(mesh: &Mesh, m: &Matrix) -> PartStats {
        silent(|c| part_stats(mesh, m, c)).unwrap()
    }

    #[test]
    fn a_cube_reports_its_size_volume_area_and_counts() {
        let cube = make_box([0.0; 3], [20.0; 3]);
        let s = stats_of(&cube, &translate(10.0, 20.0, 0.0));
        assert_eq!([s.triangles, s.edges, s.points, s.shells], [12, 18, 8, 1]);
        assert_eq!(s.volume, Some(8000.0));
        assert_eq!(s.area, 2400.0);
        assert_eq!(s.min, Some([10.0, 20.0, 0.0]));
        assert_eq!(s.max, Some([30.0, 40.0, 20.0]));
        assert!(s.closed && s.oriented);
        assert_eq!([s.holes, s.border_edges, s.inverted_triangles], [0, 0, 0]);
    }

    #[test]
    fn scaling_changes_volume_and_area_like_the_information_panel() {
        let cube = make_box([0.0; 3], [10.0; 3]);
        // Uniform scale 2 with a rotation of 90° about Z: similarity, area × 4, volume × 8.
        let m: Matrix = [
            0.0, 2.0, 0.0, 0.0, -2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let s = stats_of(&cube, &m);
        assert_eq!(s.volume, Some(8000.0));
        assert_eq!(s.area, 2400.0);
        // Stretch X only: no similarity, so the transformed triangles are measured.
        let mut stretch = IDENTITY;
        stretch[0] = 3.0;
        let s = stats_of(&cube, &stretch);
        assert!((s.volume.unwrap() - 3000.0).abs() < 1e-9);
        assert!((s.area - (2.0 * 300.0 + 2.0 * 300.0 + 2.0 * 100.0)).abs() < 1e-9);
        assert_eq!(s.max, Some([30.0, 10.0, 10.0]));
    }

    #[test]
    fn an_open_part_has_no_volume_and_says_why() {
        let mut open = make_box([0.0; 3], [10.0; 3]);
        open.indices.truncate(30);
        let s = stats_of(&open, &IDENTITY);
        assert_eq!(s.volume, None);
        assert!(!s.closed);
        assert_eq!([s.holes, s.border_edges], [1, 4]);
    }

    #[test]
    fn packing_density_counts_the_parts_on_the_platform_only() {
        let cube = make_box([0.0; 3], [10.0; 3]);
        let tall = make_box([0.0; 3], [10.0, 10.0, 50.0]);
        let parts = vec![
            stats_of(&cube, &translate(0.0, 0.0, 0.0)),
            stats_of(&tall, &translate(20.0, 0.0, 0.0)),
            // Sticks out past X = 100.
            stats_of(&cube, &translate(95.0, 0.0, 0.0)),
        ];
        let platform = [100.0, 100.0, 100.0];
        let all = summary(&parts, &[0, 1, 2], platform);
        assert_eq!([all.parts, all.on_platform], [3, 2]);
        assert_eq!(all.min, Some([0.0, 0.0, 0.0]));
        assert_eq!(all.max, Some([105.0, 10.0, 50.0]));
        assert_eq!(all.volume, Some(6000.0));
        assert_eq!(all.area, 600.0 + 2200.0);
        assert_eq!(all.top, Some(50.0));
        // 6000 / 1,000,000 and 6000 / (100 × 100 × 50).
        assert!((all.packing_density.unwrap() - 0.6).abs() < 1e-12);
        assert!((all.with_object_height.unwrap() - 1.2).abs() < 1e-12);

        let one = summary(&parts, &[0], platform);
        assert_eq!(one.volume, Some(1000.0));
        assert!((one.with_object_height.unwrap() - 1.0).abs() < 1e-12);
        let none = summary(&parts, &[], platform);
        assert_eq!([none.parts, none.on_platform], [0, 0]);
        assert_eq!((none.min, none.volume, none.top), (None, Some(0.0), None));
        assert_eq!(none.with_object_height, None);
    }

    #[test]
    fn an_open_part_on_the_platform_leaves_the_total_volume_unknown() {
        let cube = make_box([0.0; 3], [10.0; 3]);
        let mut open = cube.clone();
        open.indices.truncate(30);
        let parts = vec![
            stats_of(&cube, &IDENTITY),
            stats_of(&open, &translate(20.0, 0.0, 0.0)),
        ];
        let s = summary(&parts, &[0, 1], [100.0; 3]);
        assert_eq!((s.volume, s.packing_density), (None, None));
        assert_eq!(s.area, 600.0 + 500.0);
    }

    #[test]
    fn cancel_stops_a_part() {
        let cube = make_box([0.0; 3], [10.0; 3]);
        let cancel = std::sync::atomic::AtomicBool::new(true);
        let mut control = Control::new(&cancel, |_| {});
        assert!(part_stats(&cube, &IDENTITY, &mut control).is_err());
    }
}
