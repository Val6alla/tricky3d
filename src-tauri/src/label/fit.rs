//! A new label's starting size: as large as fits on the flat area round the point clicked.
//!
//! The flat area is the part's surface joined edge to edge to the triangle clicked, as far as its
//! triangles face within `FLAT_DEGREES` of the clicked normal. In the label's frame it is a region
//! of the plane round the origin; the largest label frame centred there (the label's width ÷
//! height fixed) that no edge of the region crosses gives the room, and the label takes `SHARE`
//! of it, no taller than `MAX_HEIGHT` and no shorter than `MIN_HEIGHT`. The same pass measures
//! how thick the part is under the point, for how deep a sunk label may start.

use std::collections::{HashMap, VecDeque};

use serde::Serialize;

use super::emboss::Frame;
use crate::mesh::{Mesh, cross, dot, sub};
use crate::repair::exact::{P2, P3, orient2d};

/// Tallest starting label, mm.
pub const MAX_HEIGHT: f64 = 10.0;
/// Shortest label, mm: where even this does not fit, the label still starts at it.
pub const MIN_HEIGHT: f64 = 1.0;
/// Share of the room a label starts with.
pub const SHARE: f64 = 0.8;
/// Triangles facing further than this from the clicked normal end the flat area.
pub const FLAT_DEGREES: f64 = 30.0;

/// `FitReply` in src/label/ipc.ts.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fit {
    /// The starting height, mm.
    pub height: f64,
    /// False when not even `MIN_HEIGHT` fits: the label starts at it and the window says it runs
    /// over an edge.
    pub fits: bool,
    /// How far the part reaches below the point along the normal, mm; None when nothing is
    /// below it (an open part).
    pub thickness: Option<f64>,
}

/// The starting height of a label `aspect` (width ÷ height) wide at `frame` on `mesh` (world
/// space).
pub(crate) fn fit(mesh: &Mesh, frame: &Frame, aspect: f64) -> Fit {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        1.0
    };
    // Room beyond this is never needed: the label stops at MAX_HEIGHT.
    let cap = MAX_HEIGHT / SHARE;
    let reach = [cap * aspect / 2.0 + 1.0, cap / 2.0 + 1.0];
    let tris: &[[u32; 3]] = mesh.indices.as_chunks::<3>().0;
    let loc: Vec<P3> = (0..mesh.vertex_count() as u32)
        .map(|i| frame.local(mesh.point(i)))
        .collect();
    let flat = |i: u32| -> P2 { [loc[i as usize][0], loc[i as usize][1]] };
    let near: Vec<u32> = (0..tris.len() as u32)
        .filter(|&t| {
            let p = tris[t as usize].map(flat);
            let (u0, u1) = (
                p.iter().map(|q| q[0]).fold(f64::INFINITY, f64::min),
                p.iter().map(|q| q[0]).fold(f64::NEG_INFINITY, f64::max),
            );
            let (v0, v1) = (
                p.iter().map(|q| q[1]).fold(f64::INFINITY, f64::min),
                p.iter().map(|q| q[1]).fold(f64::NEG_INFINITY, f64::max),
            );
            u0 <= reach[0] && u1 >= -reach[0] && v0 <= reach[1] && v1 >= -reach[1]
        })
        .collect();
    let normal = |t: u32| -> P3 {
        let [a, b, c] = tris[t as usize].map(|i| loc[i as usize]);
        cross(sub(b, a), sub(c, a))
    };
    let flat_enough = |t: u32| {
        let n = normal(t);
        n[2] > FLAT_DEGREES.to_radians().cos() * dot(n, n).sqrt()
    };
    // Over the origin, facing either way (the part's far side faces away).
    let contains_origin = |t: u32| {
        let [a, b, c] = tris[t as usize].map(flat);
        let o = [0.0, 0.0];
        let s = [orient2d(a, b, o), orient2d(b, c, o), orient2d(c, a, o)];
        s.iter().all(|&x| x >= 0) || s.iter().all(|&x| x <= 0)
    };
    // The surface under the point: the facing triangle over the origin nearest the plane.
    let at_origin = |t: u32| height_at(tris[t as usize].map(|i| loc[i as usize]));
    let seed = near
        .iter()
        .copied()
        .filter(|&t| normal(t)[2] > 0.0 && contains_origin(t))
        .min_by(|&a, &b| at_origin(a).abs().total_cmp(&at_origin(b).abs()));
    let Some(seed) = seed else {
        return Fit {
            height: MIN_HEIGHT,
            fits: false,
            thickness: None,
        };
    };
    // Thickness: the nearest surface under the point, further down the normal.
    let surface = at_origin(seed);
    let thickness = near
        .iter()
        .copied()
        .filter(|&t| t != seed && normal(t)[2] != 0.0 && contains_origin(t))
        .map(|t| surface - at_origin(t))
        .filter(|&d| d > 1e-6)
        .min_by(f64::total_cmp);
    // The flat area round the seed.
    let mut edge_tris: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for &t in &near {
        let [a, b, c] = tris[t as usize];
        for (p, q) in [(a, b), (b, c), (c, a)] {
            edge_tris.entry((p.min(q), p.max(q))).or_default().push(t);
        }
    }
    let mut area: HashMap<u32, ()> = HashMap::from([(seed, ())]);
    let mut queue = VecDeque::from([seed]);
    while let Some(t) = queue.pop_front() {
        let [a, b, c] = tris[t as usize];
        for (p, q) in [(a, b), (b, c), (c, a)] {
            for &n in &edge_tris[&(p.min(q), p.max(q))] {
                if !area.contains_key(&n) && flat_enough(n) {
                    area.insert(n, ());
                    queue.push_back(n);
                }
            }
        }
    }
    // The room: the frame scale at which the label frame first meets an edge of the area.
    let mut room = cap;
    for &t in area.keys() {
        let [a, b, c] = tris[t as usize];
        for (p, q) in [(a, b), (b, c), (c, a)] {
            let inner = edge_tris[&(p.min(q), p.max(q))]
                .iter()
                .any(|&n| n != t && area.contains_key(&n));
            if !inner {
                room = room.min(reached_at(flat(p), flat(q), aspect));
            }
        }
    }
    let height = (SHARE * room).min(MAX_HEIGHT);
    Fit {
        height: if room < MIN_HEIGHT {
            MIN_HEIGHT
        } else {
            height.max(MIN_HEIGHT)
        },
        fits: room >= MIN_HEIGHT,
        thickness,
    }
}

/// The height of the plane through `t` over the frame's origin.
fn height_at(t: [P3; 3]) -> f64 {
    let [a, b, c] = t.map(|q| [q[0], q[1]]);
    let cross2 = |p: P2, q: P2| p[0] * q[1] - p[1] * q[0];
    let s = |p: P2, q: P2| [p[0] - q[0], p[1] - q[1]];
    let area = cross2(s(b, a), s(c, a));
    let o = [0.0, 0.0];
    let beta = cross2(s(o, a), s(c, a)) / area;
    let gamma = cross2(s(b, a), s(o, a)) / area;
    t[0][2] + beta * (t[1][2] - t[0][2]) + gamma * (t[2][2] - t[0][2])
}

/// The smallest height at which a frame of `aspect`, centred on the origin, reaches the segment
/// `a`–`b`: the least over the segment of max(2|u| ÷ aspect, 2|v|). That is convex along the
/// segment and straight between the points where either term is zero or the two are equal, so
/// its least value is at one of those or at an end.
fn reached_at(a: P2, b: P2, aspect: f64) -> f64 {
    let f = |p: P2| [2.0 * p[0] / aspect, 2.0 * p[1]];
    let (fa, fb) = (f(a), f(b));
    let d = [fb[0] - fa[0], fb[1] - fa[1]];
    let at = |t: f64| (fa[0] + t * d[0]).abs().max((fa[1] + t * d[1]).abs());
    let mut ts = vec![0.0, 1.0];
    let mut root = |num: f64, den: f64| {
        if den.abs() > 1e-15 {
            ts.push(num / den);
        }
    };
    root(-fa[0], d[0]);
    root(-fa[1], d[1]);
    root(fa[1] - fa[0], d[0] - d[1]);
    root(-(fa[0] + fa[1]), d[0] + d[1]);
    ts.into_iter()
        .filter(|t| (0.0..=1.0).contains(t))
        .map(at)
        .fold(f64::INFINITY, f64::min)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_reaches_a_segment_where_the_scaled_box_first_touches_it() {
        // A wall at u = 10 across the axis: a frame of aspect 2 touches it at height 10.
        assert!((reached_at([10.0, -5.0], [10.0, 5.0], 2.0) - 10.0).abs() < 1e-12);
        // A diagonal segment: touched by the frame's corner, where 2|u|/2 = 2|v|.
        let h = reached_at([0.0, 6.0], [6.0, 0.0], 2.0);
        assert!((h - 4.0).abs() < 1e-12, "{h}");
    }
}
