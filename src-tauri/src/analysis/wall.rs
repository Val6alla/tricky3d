//! Wall thickness: how thick the part is under each triangle, measured as "Wall at a point" measures
//! it (src/measure.rs pick_wall): straight in against the triangle's outward normal to where the ray
//! first meets the surface again. Each triangle is sampled at its centre and three points towards
//! its corners and keeps the thinnest. The acceptance test fails when more than the given share of
//! the surface is thinner than the threshold; the fast mode stops as soon as that is certain, and
//! the simple mode at the first wall thinner than the threshold.

use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::arrange::{V3, World, add, len, scale, sub};
use crate::measure::ray_triangle;
use crate::repair::bvh::{Bvh, boxes_of};
use crate::repair::{Control, RepairError, RepairResult};

/// `WallSettings` in src/analysis/types.ts.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WallSettings {
    /// mm: walls thinner than this are too thin.
    pub threshold: f64,
    /// Percent of the surface that may be too thin before the test fails.
    pub acceptance: f64,
    /// Stop as soon as the test fails (a fast yes or no; the colours are then incomplete).
    pub stop_early: bool,
    /// Stop at the first wall thinner than the threshold (projects saved before it have none).
    #[serde(default)]
    pub simple: bool,
}

/// `WallResult` in src/analysis/types.ts.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WallResult {
    pub threshold: f64,
    pub acceptance: f64,
    pub passed: bool,
    /// False when the simple mode stopped at a thin wall before the test could tell: `passed` is
    /// then false but the test is undecided.
    pub decided: bool,
    /// The simple mode stopped at the first thin wall.
    pub stopped_at_thin: bool,
    /// Percent of the surface measured thinner than the threshold (of the whole surface; with an
    /// early stop, at least this much).
    pub thin_percent: f64,
    /// mm².
    pub thin_area: f64,
    pub total_area: f64,
    /// The thinnest wall found (mm) and where: from the surface point straight in to the far side.
    pub min_thickness: Option<f64>,
    pub min_from: Option<V3>,
    pub min_to: Option<V3>,
    /// False when the test stopped early: some triangles were not measured.
    pub complete: bool,
    /// Triangles with no surface behind them (an open part there).
    pub unmeasured: u64,
}

/// Thickness per triangle (NaN: not measured or nothing behind it), with the result.
pub struct WallDetail {
    pub result: WallResult,
    pub thickness: Vec<f32>,
}

/// Where a triangle is sampled, as weights of its corners.
const SAMPLES: [[f64; 3]; 4] = [
    [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0],
    [2.0 / 3.0, 1.0 / 6.0, 1.0 / 6.0],
    [1.0 / 6.0, 2.0 / 3.0, 1.0 / 6.0],
    [1.0 / 6.0, 1.0 / 6.0, 2.0 / 3.0],
];

/// Triangles measured between two looks at the cancel flag and the progress.
const BATCH: usize = 16_384;

struct Index<'a> {
    world: &'a World,
    bvh: Bvh,
    reach: f64,
    eps: f64,
}

impl Index<'_> {
    /// The thinnest of the triangle's samples: (thickness, from, to), or None when nothing is
    /// behind any of them.
    fn thickness(&self, t: usize) -> Option<(f64, V3, V3)> {
        let tri = self.world.triangles[t];
        let (n, area) = self.world.normal_area(tri);
        if area <= 0.0 {
            return None;
        }
        let [a, b, c] = self.world.corners(tri);
        let inward = scale(n, -1.0);
        let mut best: Option<(f64, V3, V3)> = None;
        for w in SAMPLES {
            let p = add(add(scale(a, w[0]), scale(b, w[1])), scale(c, w[2]));
            let end = add(p, scale(inward, self.reach));
            let mut hit: Option<f64> = None;
            self.bvh.segment(p, end, |u| {
                if u as usize == t {
                    return;
                }
                let [ua, ub, uc] = self.world.corners(self.world.triangles[u as usize]);
                if let Some(s) = ray_triangle(p, inward, ua, ub, uc)
                    && s > self.eps
                    && hit.is_none_or(|h| s < h)
                {
                    hit = Some(s);
                }
            });
            if let Some(s) = hit
                && best.is_none_or(|(d, _, _)| s < d)
            {
                best = Some((s, p, add(p, scale(inward, s))));
            }
        }
        best
    }
}

pub fn wall_thickness(
    world: &World,
    s: &WallSettings,
    control: &mut Control,
) -> RepairResult<WallDetail> {
    let n = world.triangles.len();
    let boxes = boxes_of(&world.points, &world.triangles);
    let bvh = Bvh::new(&boxes);
    let diagonal = world
        .bounds()
        .map_or(0.0, |b| len(sub(b[1], b[0])))
        .max(1e-9);
    let index = Index {
        world,
        bvh,
        reach: 2.0 * diagonal,
        eps: 1e-9 * diagonal,
    };
    let areas: Vec<f64> = world
        .triangles
        .iter()
        .map(|&t| world.normal_area(t).1)
        .collect();
    let total_area: f64 = areas.iter().sum();
    let limit = total_area * s.acceptance / 100.0;
    let cancel: &AtomicBool = control.cancel_flag();
    let mut thickness = vec![f32::NAN; n];
    let mut thin_area = 0.0;
    let mut min: Option<(f64, V3, V3)> = None;
    let mut unmeasured = 0u64;
    let mut done = 0;
    let mut stopped = false;
    let mut stopped_at_thin = false;
    while done < n {
        control.check()?;
        // Small batches in the fast and simple modes, so they stop soon after the test fails or
        // the first thin wall.
        let batch = if s.stop_early || s.simple {
            BATCH / 16
        } else {
            BATCH
        };
        let end = (done + batch * rayon::current_num_threads().max(1)).min(n);
        let first = done;
        let found: Vec<Option<(f64, V3, V3)>> = (done..end)
            .into_par_iter()
            .map(|t| {
                if cancel.load(Ordering::Relaxed) {
                    None
                } else {
                    index.thickness(t)
                }
            })
            .collect();
        if cancel.load(Ordering::Relaxed) {
            return Err(RepairError::Cancelled);
        }
        let mut thin_seen = false;
        for (k, f) in found.into_iter().enumerate() {
            let t = first + k;
            match f {
                Some((d, from, to)) => {
                    thickness[t] = d as f32;
                    if d < s.threshold {
                        thin_area += areas[t];
                        thin_seen = true;
                    }
                    if min.is_none_or(|(m, _, _)| d < m) {
                        min = Some((d, from, to));
                    }
                }
                None if areas[t] > 0.0 => unmeasured += 1,
                None => {}
            }
        }
        done = end;
        control.progress(done as f64 / n as f64);
        if s.stop_early && thin_area > limit && done < n {
            stopped = true;
            break;
        }
        if s.simple && thin_seen && done < n {
            stopped = true;
            stopped_at_thin = true;
            break;
        }
    }
    let percent = if total_area > 0.0 {
        100.0 * thin_area / total_area
    } else {
        0.0
    };
    Ok(WallDetail {
        result: WallResult {
            threshold: s.threshold,
            acceptance: s.acceptance,
            passed: thin_area <= limit && !stopped_at_thin,
            decided: !stopped_at_thin || thin_area > limit,
            stopped_at_thin,
            thin_percent: percent,
            thin_area,
            total_area,
            min_thickness: min.map(|m| m.0),
            min_from: min.map(|m| m.1),
            min_to: min.map(|m| m.2),
            complete: !stopped,
            unmeasured,
        },
        thickness,
    })
}
