//! Metric screws and nuts (ISO coarse threads, M2 to M24). Both use the ISO basic thread profile:
//! a 60° thread with a flat crest of P/8 at the major diameter and a flat root of P/4, 5H/8 deep
//! (H = P √3 / 2). The nut's thread is the screw's moved outwards by the clearance, sampled at
//! the same angles and heights, so a nut at its clearance never touches its screw.
//!
//! Head and nut sizes follow ISO 4762 (hexagon socket head), ISO 4017 (hexagon head), ISO 1207
//! (slotted cheese head, extended past M10 in proportion) and ISO 4032 (nuts).

use std::f64::consts::TAU;

use super::Polygon;
use super::build::{Builder, Cap, P, extrude};
use super::solid::union;
use crate::mesh::Mesh;
use crate::repair::{Control, RepairError, RepairResult};

/// A metric size: thread and the standard heads and nut that go with it (mm).
#[derive(Clone, Copy, Debug)]
pub struct Metric {
    pub name: &'static str,
    /// Nominal (major) diameter.
    pub d: f64,
    pub pitch: f64,
    /// Across flats of the hexagon head and the nut (wrench size).
    pub wrench: f64,
    pub nut_height: f64,
    pub hex_head_height: f64,
    pub socket_head_d: f64,
    pub socket_head_height: f64,
    /// Across flats of the hexagon socket.
    pub socket: f64,
    pub slot_head_d: f64,
    pub slot_head_height: f64,
    pub slot_width: f64,
    pub slot_depth: f64,
}

#[allow(clippy::too_many_arguments)]
const fn m(
    name: &'static str,
    d: f64,
    pitch: f64,
    wrench: f64,
    nut_height: f64,
    hex_head_height: f64,
    socket_head_d: f64,
    socket: f64,
    slot_head_d: f64,
    slot_head_height: f64,
    slot_width: f64,
    slot_depth: f64,
) -> Metric {
    Metric {
        name,
        d,
        pitch,
        wrench,
        nut_height,
        hex_head_height,
        socket_head_d,
        socket_head_height: d,
        socket,
        slot_head_d,
        slot_head_height,
        slot_width,
        slot_depth,
    }
}

/// The sizes the library offers.
pub const METRIC: [Metric; 12] = [
    m("M2", 2.0, 0.4, 4.0, 1.6, 1.4, 3.8, 1.5, 3.8, 1.3, 0.5, 0.6),
    m(
        "M2.5", 2.5, 0.45, 5.0, 2.0, 1.7, 4.5, 2.0, 4.5, 1.6, 0.6, 0.7,
    ),
    m("M3", 3.0, 0.5, 5.5, 2.4, 2.0, 5.5, 2.5, 5.5, 2.0, 0.8, 0.85),
    m("M4", 4.0, 0.7, 7.0, 3.2, 2.8, 7.0, 3.0, 7.0, 2.6, 1.2, 1.1),
    m("M5", 5.0, 0.8, 8.0, 4.7, 3.5, 8.5, 4.0, 8.5, 3.3, 1.2, 1.3),
    m(
        "M6", 6.0, 1.0, 10.0, 5.2, 4.0, 10.0, 5.0, 10.0, 3.9, 1.6, 1.6,
    ),
    m(
        "M8", 8.0, 1.25, 13.0, 6.8, 5.3, 13.0, 6.0, 13.0, 5.0, 2.0, 2.0,
    ),
    m(
        "M10", 10.0, 1.5, 16.0, 8.4, 6.4, 16.0, 8.0, 16.0, 6.0, 2.5, 2.4,
    ),
    m(
        "M12", 12.0, 1.75, 18.0, 10.8, 7.5, 18.0, 10.0, 18.0, 7.2, 3.0, 2.9,
    ),
    m(
        "M16", 16.0, 2.0, 24.0, 14.8, 10.0, 24.0, 14.0, 24.0, 9.6, 4.0, 3.8,
    ),
    m(
        "M20", 20.0, 2.5, 30.0, 18.0, 12.5, 30.0, 17.0, 30.0, 12.0, 5.0, 4.8,
    ),
    m(
        "M24", 24.0, 3.0, 36.0, 21.5, 15.0, 36.0, 19.0, 36.0, 14.4, 6.0, 5.8,
    ),
];

pub(crate) fn metric(name: &str) -> &'static Metric {
    METRIC
        .iter()
        .find(|m| m.name == name)
        .expect("the choice only offers listed sizes")
}

/// The radius of the basic profile at axial phase `u` (any real; one crest every pitch, centred on
/// u = 0, k·P), for a thread of major diameter `d`.
pub(crate) fn profile(d: f64, pitch: f64, u: f64) -> f64 {
    let h = pitch * 3f64.sqrt() / 2.0;
    let major = d / 2.0;
    let w = (u / pitch + 0.5).rem_euclid(1.0) - 0.5;
    let a = w.abs() * pitch;
    if a <= pitch / 16.0 {
        major
    } else if a >= 3.0 * pitch / 8.0 {
        major - 5.0 * h / 8.0
    } else {
        // The flank rises 60° from the axis: √3 mm of depth per mm along it.
        major - (a - pitch / 16.0) * 3f64.sqrt()
    }
}

/// Points per turn, a multiple of 12 so hexagon corners land on them.
pub(crate) fn per_turn(asked: usize) -> usize {
    (asked.div_ceil(12) * 12).max(24)
}

/// A right-hand thread's radius at angle `theta` and height `z`: the profile at the height the
/// helix has climbed to.
fn thread_radius(d: f64, pitch: f64, theta: f64, z: f64) -> f64 {
    profile(d, pitch, z - pitch * theta / TAU)
}

/// Heights from `z0` to `z1` about `step` apart, both ends included.
fn heights(z0: f64, z1: f64, step: f64) -> Vec<f64> {
    let n = ((z1 - z0) / step).ceil().max(1.0) as usize;
    (0..=n)
        .map(|i| z0 + (z1 - z0) * i as f64 / n as f64)
        .collect()
}

/// A ring at height `z` whose radius at angle k·360°/n is `radius(angle)`.
fn ring(n: usize, z: f64, radius: impl Fn(f64) -> f64) -> Vec<P> {
    (0..n)
        .map(|k| {
            let a = TAU * k as f64 / n as f64;
            let r = radius(a);
            [r * a.cos(), r * a.sin(), z]
        })
        .collect()
}

/// The radius at angle `a` of a regular hexagon `across` flats with corners at 0°, 60°, …
fn hexagon(across: f64, a: f64) -> f64 {
    // The flats face 30°, 90°, …: the angle from the one nearest, then the distance to it.
    let to_flat = a.rem_euclid(TAU / 6.0) - TAU / 12.0;
    across / 2.0 / to_flat.cos()
}

/// Thread rows a pitch apart are sampled this many times.
pub(crate) const ROWS_PER_PITCH: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Head {
    Socket,
    Hexagon,
    Slotted,
}

/// A screw of `size` with `length` of thread under a head of kind `head`: the tip at z = 0 and
/// the head on top, `asked_per_turn` points round (made a multiple of 12).
pub(crate) fn screw(
    size: &Metric,
    length: f64,
    head: Head,
    asked_per_turn: usize,
    control: &mut Control,
) -> RepairResult<Mesh> {
    let n = per_turn(asked_per_turn);
    let (d, p) = (size.d, size.pitch);
    let step = p / ROWS_PER_PITCH as f64;
    let mut rings: Vec<Vec<P>> = heights(0.0, length, step)
        .into_iter()
        .map(|z| ring(n, z, |a| thread_radius(d, p, a, z)))
        .collect();
    let top = |head_d: f64, head_h: f64, rings: &mut Vec<Vec<P>>| {
        let r = head_d / 2.0;
        rings.push(ring(n, length, |_| r));
        rings.push(ring(n, length + head_h, |_| r));
    };
    match head {
        Head::Hexagon => {
            let w = size.wrench;
            rings.push(ring(n, length, |a| hexagon(w, a)));
            rings.push(ring(n, length + size.hex_head_height, |a| hexagon(w, a)));
        }
        Head::Socket => {
            top(size.socket_head_d, size.socket_head_height, &mut rings);
            // The socket: inwards at the top, then down 60 % of the head.
            let z_top = length + size.socket_head_height;
            let depth = 0.6 * size.socket_head_height;
            let s = size.socket;
            rings.push(ring(n, z_top, |a| hexagon(s, a)));
            rings.push(ring(n, z_top - depth, |a| hexagon(s, a)));
        }
        Head::Slotted => {
            // Up to the bottom of the slot; the two cheeks either side of it are joined on below.
            top(
                size.slot_head_d,
                size.slot_head_height - size.slot_depth,
                &mut rings,
            );
        }
    }
    control.check()?;
    let mut b = Builder::default();
    b.stack(&rings, Cap::Flat, Cap::Flat);
    let body = b.finish_in_place().map_err(RepairError::Failed)?;
    if head != Head::Slotted {
        return Ok(body);
    }
    // Each cheek: the part of the head's circle beyond the slot, from a little below the slot's
    // floor to the top, overlapping the body so the two join.
    let r = size.slot_head_d / 2.0;
    let half = size.slot_width / 2.0;
    let z0 = length + size.slot_head_height - size.slot_depth - 0.2 * size.slot_depth;
    let z1 = length + size.slot_head_height;
    let mut parts = vec![body];
    for side in [1.0, -1.0] {
        let start = (half / r).asin();
        let outline: Vec<[f64; 2]> = (0..=n / 2)
            .map(|k| {
                let a = start + (std::f64::consts::PI - 2.0 * start) * k as f64 / (n / 2) as f64;
                [r * a.cos(), side * r * a.sin()]
            })
            .collect();
        let mut cheek = Builder::default();
        extrude(&mut cheek, &[Polygon::new(outline, Vec::new())], z0, z1)
            .map_err(RepairError::Failed)?;
        parts.push(cheek.finish_in_place().map_err(RepairError::Failed)?);
    }
    union(&parts, control)
}

/// A hexagon nut for `size` with its thread `clearance` mm wider than the screw's all round, the
/// standard height, bottom at z = 0.
pub(crate) fn nut(size: &Metric, clearance: f64, asked_per_turn: usize) -> Result<Mesh, String> {
    let n = per_turn(asked_per_turn);
    let (d, p) = (size.d, size.pitch);
    let h = size.nut_height;
    let step = p / ROWS_PER_PITCH as f64;
    let inner = |z: f64| ring(n, z, |a| thread_radius(d, p, a, z) + clearance);
    // Round the cross-section: up the bore, out across the top, down the flats, in across the
    // bottom back to the bore.
    let mut rings: Vec<Vec<P>> = heights(0.0, h, step).into_iter().map(inner).collect();
    let w = size.wrench;
    rings.push(ring(n, h, |a| hexagon(w, a)));
    rings.push(ring(n, 0.0, |a| hexagon(w, a)));
    let mut b = Builder::default();
    b.closed_stack(&rings, 0);
    b.finish_in_place()
}
