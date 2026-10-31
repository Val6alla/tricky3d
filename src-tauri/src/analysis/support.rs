//! Support volume: an estimate of the support a part needs, as the volume of solid blocks from
//! every downskin (triangles facing down, tilted against the X/Y plane by at most the threshold
//! angle) straight down to the platform (Z = 0). What is under the platform does not count.

use serde::{Deserialize, Serialize};

use super::tilt_degrees;
use crate::arrange::{V3, World};
use crate::mesh::Mesh;
use crate::repair::topology::Edges;
use crate::repair::{Control, RepairResult};

/// `SupportSettings` in src/analysis/types.ts.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SupportSettings {
    /// Degrees, 0 to 90.
    pub angle: f64,
}

/// `SupportResult` in src/analysis/types.ts.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Support {
    pub angle: f64,
    /// mm³.
    pub volume: f64,
    /// mm², the downskins above the platform seen from below.
    pub area: f64,
    pub triangles: u64,
}

/// The support estimate, the triangles that need support (1) and the blocks to draw: world
/// triangles (x, y, z per corner) of the blocks' bottoms on the platform and their side walls.
pub struct SupportDetail {
    pub support: Support,
    pub classes: Vec<u8>,
    pub blocks: Vec<f32>,
}

/// How high above the platform (mm) a downskin still counts as resting on it.
const RESTING: f64 = 1e-6;

/// The part of triangle `t` at or above Z = 0, as a fan of triangles.
fn above_platform(t: [V3; 3]) -> Vec<[V3; 3]> {
    let mut poly: Vec<V3> = Vec::with_capacity(4);
    for i in 0..3 {
        let (p, q) = (t[i], t[(i + 1) % 3]);
        if p[2] >= 0.0 {
            poly.push(p);
        }
        if (p[2] >= 0.0) != (q[2] >= 0.0) {
            let s = p[2] / (p[2] - q[2]);
            poly.push([p[0] + s * (q[0] - p[0]), p[1] + s * (q[1] - p[1]), 0.0]);
        }
    }
    (1..poly.len().saturating_sub(1))
        .map(|i| [poly[0], poly[i], poly[i + 1]])
        .collect()
}

/// The projected (X/Y) area of a triangle.
fn flat_area(t: &[V3; 3]) -> f64 {
    let [a, b, c] = t;
    ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.0
}

pub fn support_volume(
    mesh: &Mesh,
    world: &World,
    s: &SupportSettings,
    control: &mut Control,
) -> RepairResult<SupportDetail> {
    let n = world.triangles.len();
    let mut classes = vec![0u8; n];
    let mut volume = 0.0;
    let mut area = 0.0;
    let mut count = 0u64;
    let mut blocks = Vec::new();
    let mut push = |p: V3| blocks.extend_from_slice(&[p[0] as f32, p[1] as f32, p[2] as f32]);
    let floor = |p: V3| [p[0], p[1], 0.0];
    for (i, &t) in world.triangles.iter().enumerate() {
        if i % 65_536 == 0 {
            control.check()?;
            control.progress(0.8 * i as f64 / n.max(1) as f64);
        }
        let (normal, a) = world.normal_area(t);
        if a <= 0.0 || normal[2] >= 0.0 || tilt_degrees(normal[2]) > s.angle + 1e-9 {
            continue;
        }
        let corners = world.corners(t);
        // Resting on the platform (or under it): nothing to hold up.
        if corners.iter().all(|p| p[2] <= RESTING) {
            continue;
        }
        classes[i] = 1;
        count += 1;
        for piece in above_platform(corners) {
            area += flat_area(&piece);
            let mean = (piece[0][2] + piece[1][2] + piece[2][2]) / 3.0;
            volume += flat_area(&piece) * mean;
            // The block's bottom, facing down: the piece's corners on the platform as they are
            // (the piece faces down, so seen from below it winds the same way).
            for p in piece {
                push(floor(p));
            }
        }
    }
    control.check()?;
    // Side walls where a block ends: edges with a support triangle on one side only.
    let edges = Edges::new(mesh);
    for e in 0..edges.len() as u32 {
        let sides = edges.sides(e);
        let supported = sides
            .iter()
            .filter(|&&side| classes[(side / 3) as usize] == 1)
            .count();
        if supported == 0 || supported == sides.len() {
            continue;
        }
        // The view draws the walls from both sides, so their winding does not matter.
        let [p, q] = edges.ends(e).map(|v| world.points[v as usize]);
        if p[2] <= 0.0 && q[2] <= 0.0 {
            continue;
        }
        let (p, q) = ([p[0], p[1], p[2].max(0.0)], [q[0], q[1], q[2].max(0.0)]);
        for c in [p, floor(q), q, p, floor(p), floor(q)] {
            push(c);
        }
    }
    control.progress(1.0);
    Ok(SupportDetail {
        support: Support {
            angle: s.angle,
            volume,
            area,
            triangles: count,
        },
        classes,
        blocks,
    })
}
