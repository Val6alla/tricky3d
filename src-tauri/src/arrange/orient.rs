//! Turning a part: Lay flat on a clicked face or on its largest flat face, Rotate to smallest box,
//! and Orient, which ranks the ways a part can stand on the platform by support area, height or
//! footprint. Each gives a `Turn`: a rotation about the part's box centre and a move after it.

use std::collections::HashMap;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::hull::{self, Hull};
use super::{
    IDENTITY_ROT, Rot, V3, World, add, apply, bounds, centre, compose, cross, dot, len, rot_z,
    rotated_about, rotation_between, scale, sub, translated, unit,
};
use crate::mesh::Matrix;
use crate::repair::{Control, RepairResult};

/// Faces whose normals are within this many degrees of each other count as one flat face.
pub const FLAT_DEGREES: f64 = 3.0;
/// Surfaces facing down steeper than this many degrees below horizontal need support.
pub const CRITICAL_DEGREES: f64 = 45.0;
/// How many ways of standing Orient ranks: the largest groups of hull faces.
const ORIENT_CANDIDATES: usize = 48;
/// How many of them it gives back.
pub const ORIENT_SHOWN: usize = 5;

const DOWN: V3 = [0.0, 0.0, -1.0];

/// A world-space rotation of a part about `pivot`, then a move by `shift`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Turn {
    pub rotation: Rot,
    pub pivot: V3,
    pub shift: V3,
}

impl Turn {
    /// The part's new matrix.
    pub fn apply(&self, m: &Matrix) -> Matrix {
        translated(&rotated_about(m, &self.rotation, self.pivot), self.shift)
    }
}

/// The part where `turn` puts it.
pub fn moved(part: &World, turn: &Turn) -> World {
    let mut out = part.rotated(&turn.rotation, turn.pivot);
    for p in &mut out.points {
        *p = add(*p, turn.shift);
    }
    out
}

/// The highest point of the part where `turn` puts it.
pub fn moved_top(part: &World, turn: &Turn) -> f64 {
    part.points
        .iter()
        .map(|&p| apply(&turn.rotation, sub(p, turn.pivot))[2] + turn.pivot[2] + turn.shift[2])
        .fold(f64::NEG_INFINITY, f64::max)
}

/// How the part's box sits after a turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rest {
    /// The lowest point on the platform (Z = 0), the box's centre over where it was.
    OnPlatform,
    /// The lowest point at the height it was, the box's centre over where it was.
    Same,
}

fn turn(part: &World, rotation: Rot, rest: Rest) -> Option<Turn> {
    let before = part.bounds()?;
    let pivot = centre(&before);
    let after = bounds(
        &part
            .points
            .iter()
            .map(|&p| add(apply(&rotation, sub(p, pivot)), pivot))
            .collect::<Vec<_>>(),
    )?;
    let c = centre(&after);
    let z = match rest {
        Rest::OnPlatform => 0.0,
        Rest::Same => before[0][2],
    };
    Some(Turn {
        rotation,
        pivot,
        shift: [pivot[0] - c[0], pivot[1] - c[1], z - after[0][2]],
    })
}

fn tolerance(part: &World) -> f64 {
    part.bounds()
        .map_or(1e-6, |b| 1e-6 + 1e-5 * len(sub(b[1], b[0])))
}

// ---------------------------------------------------------------------------------------------
// Flat faces

/// A flat face: triangles whose normals are within FLAT_DEGREES of each other and that lie in one
/// plane, with their area-weighted normal, total area and the plane's offset along the normal.
#[derive(Clone, Debug)]
struct Flat {
    sum: V3,
    area: f64,
    offset: f64,
    seed: V3,
}

impl Flat {
    fn normal(&self) -> V3 {
        unit(self.sum).unwrap_or(self.seed)
    }
}

/// Groups normals (with areas, and offsets when `plane_tol` is set) into flat faces, largest first.
fn group_flats(items: impl Iterator<Item = (V3, f64, f64)>, plane_tol: Option<f64>) -> Vec<Flat> {
    let cos = FLAT_DEGREES.to_radians().cos();
    // Normals are bucketed on a grid finer than the angle, so only the 27 buckets round a normal
    // can hold a face it belongs to.
    let cell = FLAT_DEGREES.to_radians();
    let key = |n: V3| n.map(|v| (v / cell).round() as i32);
    let mut flats: Vec<Flat> = Vec::new();
    let mut buckets: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
    for (n, area, offset) in items {
        if area <= 0.0 {
            continue;
        }
        let k = key(n);
        let mut found = None;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = buckets.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) else {
                        continue;
                    };
                    for &f in list {
                        let flat = &flats[f];
                        if dot(flat.seed, n) >= cos
                            && plane_tol.is_none_or(|t| (offset - flat.offset).abs() <= t)
                        {
                            found = Some(f);
                            break 'search;
                        }
                    }
                }
            }
        }
        match found {
            Some(f) => {
                let flat = &mut flats[f];
                flat.sum = add(flat.sum, scale(n, area));
                flat.area += area;
            }
            None => {
                buckets.entry(k).or_default().push(flats.len());
                flats.push(Flat {
                    sum: scale(n, area),
                    area,
                    offset,
                    seed: n,
                });
            }
        }
    }
    flats.sort_by(|a, b| b.area.total_cmp(&a.area));
    flats
}

/// The part's triangles grouped into flat faces, largest first.
fn mesh_flats(part: &World, tol: f64) -> Vec<Flat> {
    group_flats(
        part.triangles.iter().map(|&t| {
            let (n, area) = part.normal_area(t);
            let [a, b, c] = part.corners(t);
            (n, area, dot(n, scale(add(add(a, b), c), 1.0 / 3.0)))
        }),
        Some(tol),
    )
}

/// The hull's faces grouped by direction, largest first: the ways the part can rest on the platform.
fn hull_flats(hull: &Hull) -> Vec<Flat> {
    group_flats(
        hull.face_normals().into_iter().map(|(n, a)| (n, a, 0.0)),
        None,
    )
}

fn hull_or_points(part: &World) -> Vec<V3> {
    match hull::hull(&part.points) {
        Some(h) => h.points,
        None => part.points.clone(),
    }
}

/// Lay flat on a clicked face: the flat face round `point` facing `normal` (both world space) goes
/// down onto the platform. None for a part with no geometry.
pub fn lay_flat_at(part: &World, point: V3, normal: V3) -> Option<Turn> {
    let normal = unit(normal)?;
    let tol = tolerance(part);
    let cos = FLAT_DEGREES.to_radians().cos();
    let mut sum = [0.0; 3];
    for &t in &part.triangles {
        let (n, area) = part.normal_area(t);
        if area <= 0.0 || dot(n, normal) < cos {
            continue;
        }
        let [a, b, c] = part.corners(t);
        if [a, b, c]
            .iter()
            .all(|&p| dot(normal, sub(p, point)).abs() <= tol)
        {
            sum = add(sum, scale(n, area));
        }
    }
    let face = unit(sum).unwrap_or(normal);
    turn(part, rotation_between(face, DOWN), Rest::OnPlatform)
}

/// Lay flat automatically: the largest flat face the part can rest on (one at the outside, with
/// nothing of the part below it) goes down onto the platform. A part with no flat face worth the
/// name (none of at least FLAT_SHARE of its surface, like a torus or a sphere) rests on the largest
/// face of its convex hull instead: a torus lies flat rather than standing on its rim.
pub fn lay_flat_auto(part: &World) -> Option<Turn> {
    let tol = tolerance(part);
    let flats = mesh_flats(part, tol);
    let surface: f64 = flats.iter().map(|f| f.area).sum();
    let outside = hull_or_points(part);
    let face = flats
        .iter()
        .take(4000)
        .take_while(|f| f.area >= FLAT_SHARE * surface)
        .find(|f| {
            let n = f.normal();
            let top = outside.iter().map(|&p| dot(n, p)).fold(f64::MIN, f64::max);
            f.offset >= top - tol
        })
        .map(Flat::normal)
        .or_else(|| {
            let h = hull::hull(&part.points)?;
            hull_flats(&h).first().map(Flat::normal)
        })
        .or_else(|| flats.first().map(Flat::normal))?;
    turn(part, rotation_between(face, DOWN), Rest::OnPlatform)
}

/// The smallest share of a part's surface a flat face needs for Lay flat automatically to rest the
/// part on it.
const FLAT_SHARE: f64 = 0.01;

// ---------------------------------------------------------------------------------------------
// Smallest box

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BoxAxes {
    /// Turn about Z only, so the part keeps standing as it does.
    Z,
    All,
}

/// Of the four rotations with these rows up to their signs (the last row the cross product of the
/// first two), the one that turns the part least.
fn least_turn(x: V3, y: V3) -> Rot {
    let mut best = IDENTITY_ROT;
    let mut trace = f64::MIN;
    for sx in [1.0, -1.0] {
        for sy in [1.0, -1.0] {
            let (a, b) = (scale(x, sx), scale(y, sy));
            let r = [a, b, cross(a, b)];
            let t = r[0][0] + r[1][1] + r[2][2];
            if t > trace + 1e-12 {
                (best, trace) = (r, t);
            }
        }
    }
    best
}

/// The turn about Z that brings the footprint of `points` to its smallest box, longer side along X,
/// by the smaller of the two turns that do it.
fn square_about_z(points: &[V3]) -> Rot {
    let flat: Vec<[f64; 2]> = points.iter().map(|p| [p[0], p[1]]).collect();
    let (d, w, h) = hull::min_rectangle(&hull::hull2(&flat));
    let along = if w >= h { d } else { [-d[1], d[0]] };
    let mut angle = -along[1].atan2(along[0]);
    // Half a turn gives the same box; take the smaller turn.
    let half = std::f64::consts::PI;
    while angle > half / 2.0 + 1e-12 {
        angle -= half;
    }
    while angle <= -half / 2.0 - 1e-12 {
        angle += half;
    }
    rot_z(angle)
}

/// Rotate to smallest box: the part turned so its axis-aligned box is as small as it gets, about Z
/// only or about all axes. The longest side ends up along X, the shortest along Z (all axes) and
/// the part keeps its height above the platform.
pub fn min_box(part: &World, axes: BoxAxes) -> Option<Turn> {
    let outside = hull_or_points(part);
    let rotation = match axes {
        BoxAxes::Z => square_about_z(&outside),
        BoxAxes::All => {
            let mut candidates: Vec<V3> = vec![[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
            if let Some(h) = hull::hull(&part.points) {
                candidates.extend(hull_flats(&h).iter().take(256).map(Flat::normal));
            }
            let mut best: Option<(f64, [V3; 3], V3)> = None;
            for n in candidates {
                let (u, v) = hull::frame(n);
                let flat: Vec<[f64; 2]> = outside.iter().map(|&p| hull::project(p, u, v)).collect();
                let (d, w, h) = hull::min_rectangle(&hull::hull2(&flat));
                let (lo, hi) = outside
                    .iter()
                    .map(|&p| dot(n, p))
                    .fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(x), b.max(x)));
                let volume = w * h * (hi - lo);
                if best.as_ref().is_none_or(|b| volume < b.0 * (1.0 - 1e-9)) {
                    let a = hull::lift(d, u, v);
                    let b = hull::lift([-d[1], d[0]], u, v);
                    best = Some((volume, [a, b, n], [w, h, hi - lo]));
                }
            }
            let (_, dirs, sides) = best?;
            let mut order = [0, 1, 2];
            order.sort_by(|&i, &j| sides[j].total_cmp(&sides[i]));
            least_turn(dirs[order[0]], dirs[order[1]])
        }
    };
    turn(part, rotation, Rest::Same)
}

// ---------------------------------------------------------------------------------------------
// Orient

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Goal {
    LeastSupport,
    LowestHeight,
    SmallestFootprint,
}

/// How a part does standing one way: mm², mm, mm².
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scores {
    /// Area under surfaces facing down steeper than CRITICAL_DEGREES, not counting what rests on
    /// the platform, seen from below.
    pub support_area: f64,
    pub height: f64,
    /// Area of the outline the part covers on the platform (its convex hull seen from above).
    pub footprint: f64,
}

impl Scores {
    fn ranked(&self, goal: Goal) -> [f64; 3] {
        let [s, h, f] = [self.support_area, self.height, self.footprint];
        match goal {
            Goal::LeastSupport => [s, h, f],
            Goal::LowestHeight => [h, s, f],
            Goal::SmallestFootprint => [f, s, h],
        }
    }

    fn same_as(&self, other: &Scores) -> bool {
        let near = |a: f64, b: f64| (a - b).abs() <= 1e-6 * (1.0 + a.abs().max(b.abs()));
        near(self.support_area, other.support_area)
            && near(self.height, other.height)
            && near(self.footprint, other.footprint)
    }
}

/// One way to stand, with its scores.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub turn: Turn,
    pub scores: Scores,
}

/// The scores of the part standing with `down` (a unit vector) pointing at the platform.
fn scores(part: &World, outside: &[V3], down: V3, tol: f64) -> Scores {
    let (lo, hi) = outside
        .iter()
        .map(|&p| dot(down, p))
        .fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(x), b.max(x)));
    let (u, v) = hull::frame(down);
    let shadow: Vec<[f64; 2]> = outside.iter().map(|&p| hull::project(p, u, v)).collect();
    let footprint = hull::area2(&hull::hull2(&shadow)).abs();
    let steep = CRITICAL_DEGREES.to_radians().cos();
    let support_area = part
        .triangles
        .par_iter()
        .map(|&t| {
            let (n, area) = part.normal_area(t);
            let facing = dot(n, down);
            if facing < steep {
                return 0.0;
            }
            let on_platform = part.corners(t).iter().all(|&p| dot(down, p) >= hi - tol);
            if on_platform { 0.0 } else { area * facing }
        })
        .sum();
    Scores {
        support_area,
        height: hi - lo,
        footprint,
    }
}

/// The part's scores as it stands now, and up to ORIENT_SHOWN other ways to stand, best for `goal`
/// first. Ways that score the same as a better one are left out. Cancel stops it between ways.
pub fn orient(
    part: &World,
    goal: Goal,
    control: &mut Control,
) -> RepairResult<(Scores, Vec<Candidate>)> {
    let tol = tolerance(part);
    let hull = hull::hull(&part.points);
    let outside = hull
        .as_ref()
        .map_or_else(|| part.points.clone(), |h| h.points.clone());
    control.check()?;
    let now = scores(part, &outside, DOWN, tol);
    let mut downs: Vec<V3> = hull
        .as_ref()
        .map(|h| {
            hull_flats(h)
                .iter()
                .take(ORIENT_CANDIDATES)
                .map(Flat::normal)
                .collect()
        })
        .unwrap_or_default();
    if downs.is_empty() {
        downs = vec![DOWN, [0.0, 0.0, 1.0]];
    }
    let mut found: Vec<(Scores, V3)> = Vec::with_capacity(downs.len());
    for (i, &down) in downs.iter().enumerate() {
        control.check()?;
        found.push((scores(part, &outside, down, tol), down));
        control.progress((i + 1) as f64 / downs.len() as f64);
    }
    found.sort_by(|a, b| {
        let (x, y) = (a.0.ranked(goal), b.0.ranked(goal));
        x[0].total_cmp(&y[0])
            .then(x[1].total_cmp(&y[1]))
            .then(x[2].total_cmp(&y[2]))
    });
    let mut out: Vec<Candidate> = Vec::new();
    for (s, down) in found {
        if out.iter().any(|c| c.scores.same_as(&s)) {
            continue;
        }
        // Face down, then squared about Z so the part's box lines up with the platform.
        let lay = rotation_between(down, DOWN);
        let laid: Vec<V3> = outside.iter().map(|&p| apply(&lay, p)).collect();
        let rotation = compose(&square_about_z(&laid), &lay);
        let Some(turn) = turn(part, rotation, Rest::OnPlatform) else {
            continue;
        };
        out.push(Candidate { turn, scores: s });
        if out.len() == ORIENT_SHOWN {
            break;
        }
    }
    Ok((now, out))
}
