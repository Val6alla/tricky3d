//! Tag labels: a plate standing out from the part, joined to it by a thin bridge that breaks off,
//! with the caption raised from or sunk into the plate's front.
//!
//! The plate stands along the tag's direction d (the surface normal unless the window's handle
//! turned it), its face spanned by the text direction and d, so the caption reads upwards from
//! the part. The bridge leaves the surface along d for `GAP` mm (a socket: `emboss` with an open
//! top) and the plate's bottom closes round the bridge's top, sharing its points.

use super::emboss::{self, Frame, Relief, scale, unit};
use super::region::{P2, Region};
use super::{Build, Spec};
use crate::library::Polygon;
use crate::mesh::{Mesh, cross, dot, sub};
use crate::repair::exact::P3;
use crate::repair::{Control, RepairResult, tritri};

/// How far the plate stands off the surface (the bridge's length), mm.
pub(crate) const GAP: f64 = 1.0;
/// The tag may lean at most this far from the surface normal.
const MAX_LEAN_DEGREES: f64 = 70.0;

pub(crate) const LEAN: &str =
    "The tag leans too far over the surface. Drag its handle further away from the part.";
pub(crate) const HITS_PART: &str =
    "The tag runs into the part. Drag its handle to point the tag away from the surface.";

/// The plate's size round a caption of `width` × `height`: margin, plate width and height, and
/// the bridge's width and thickness.
pub(crate) fn plate_size(width: f64, height: f64, thickness: f64) -> (f64, f64, f64, f64, f64) {
    let margin = (0.2 * height).max(1.0);
    let plate_w = width + 2.0 * margin;
    let plate_h = height + 2.0 * margin;
    let bridge_w = (0.3 * plate_w).clamp(1.0, 4.0);
    let bridge_t = (0.6 * thickness).min(0.8);
    (margin, plate_w, plate_h, bridge_w, bridge_t)
}

/// The tag for `spec` on `mesh` at `frame`, its caption `text` (mm, centred, already mirrored when
/// the build type says so) `width` wide.
pub(crate) fn attach(
    mesh: &Mesh,
    frame: &Frame,
    spec: &Spec,
    text: &Region,
    width: f64,
    control: &mut Control,
) -> RepairResult<Result<Mesh, String>> {
    let d = spec.tag_direction.and_then(unit).unwrap_or(frame.w);
    if dot(d, frame.w) < MAX_LEAN_DEGREES.to_radians().cos() {
        return Ok(Err(LEAN.into()));
    }
    let Some(u) = unit(sub(frame.u, scale(d, dot(frame.u, d)))) else {
        return Ok(Err(LEAN.into()));
    };
    let tag = Frame {
        origin: frame.origin,
        u,
        v: cross(d, u),
        w: d,
    };
    let thickness = spec.thickness;
    let (_, plate_w, plate_h, bridge_w, bridge_t) = plate_size(width, spec.height, thickness);
    let bridge = Region::from_loops(vec![vec![
        [-bridge_w / 2.0, -bridge_t / 2.0],
        [bridge_w / 2.0, -bridge_t / 2.0],
        [bridge_w / 2.0, bridge_t / 2.0],
        [-bridge_w / 2.0, bridge_t / 2.0],
    ]]);
    let socket = match emboss::emboss(
        mesh,
        &tag,
        &bridge,
        Relief::Socket(GAP),
        &mut control.sub(0.0, 0.5),
    )? {
        Ok(s) => s,
        Err(why) => return Ok(Err(why)),
    };
    let [hole] = socket.top.as_slice() else {
        return Ok(Err(emboss::FAILED.into()));
    };
    let base = &socket.mesh;
    // The plate on its own: its hole's points first, so they keep their numbers through the
    // caption's emboss and can be swapped for the socket's.
    let hole_world: Vec<P3> = hole.iter().map(|&i| base.point(i)).collect();
    let plate = match plate_mesh(&tag, &hole_world, plate_w, plate_h, thickness) {
        Some(p) => p,
        None => return Ok(Err(emboss::FAILED.into())),
    };
    let face = caption_face(&tag, thickness, plate_h);
    let relief = if matches!(spec.build, Build::Add | Build::AddMirrored) {
        Relief::Raise(spec.depth)
    } else {
        Relief::Sink(spec.depth)
    };
    let plate = match emboss::emboss(&plate, &face, text, relief, &mut control.sub(0.5, 0.9))? {
        Ok(p) => p.mesh,
        Err(why) => return Ok(Err(why)),
    };
    // Joined: the plate's hole points become the socket's top.
    let offset = base.vertex_count() as u32 - hole.len() as u32;
    let mut positions = base.positions.clone();
    positions.extend_from_slice(&plate.positions[hole.len() * 3..]);
    let mut indices = base.indices.clone();
    let part_triangles = indices.len() / 3;
    indices.extend(plate.indices.iter().map(|&i| {
        if (i as usize) < hole.len() {
            hole[i as usize]
        } else {
            i + offset
        }
    }));
    let joined = Mesh { positions, indices };
    control.check()?;
    // The plate may not reach into the part: no plate triangle crosses a part triangle.
    let tris: &[[u32; 3]] = joined.indices.as_chunks::<3>().0;
    let (lo, hi) = bounds(&joined, &tris[part_triangles..]);
    let near: Vec<u32> = tris
        .iter()
        .filter(|t| {
            let ps = t.map(|i| joined.point(i));
            (0..3).all(|k| {
                ps.iter().map(|p| p[k]).fold(f64::INFINITY, f64::min) <= hi[k]
                    && ps.iter().map(|p| p[k]).fold(f64::NEG_INFINITY, f64::max) >= lo[k]
            })
        })
        .flatten()
        .copied()
        .collect();
    let local = Mesh {
        positions: joined.positions.clone(),
        indices: near,
    };
    if !tritri::intersecting_pairs(&local, &mut control.sub(0.9, 1.0))?.is_empty() {
        return Ok(Err(HITS_PART.into()));
    }
    Ok(Ok(joined))
}

/// The plate face the caption goes on. A tag standing up from a level surface carries it on its
/// front (−v), reading upwards from the part. A tag sticking out sideways from a wall lies flat
/// like a shelf: the caption goes on its upper face, reading with its top towards the wall, so it
/// is never underneath.
pub(crate) fn caption_face(tag: &Frame, thickness: f64, plate_h: f64) -> Frame {
    let middle = GAP + plate_h / 2.0;
    if tag.w[2].abs() >= SIDEWAYS.cos() {
        return Frame {
            origin: tag.world([0.0, -thickness / 2.0, middle]),
            u: tag.u,
            v: tag.w,
            w: scale(tag.v, -1.0),
        };
    }
    let side = if tag.v[2] >= 0.0 { 1.0 } else { -1.0 };
    let w = scale(tag.v, side);
    let v = scale(tag.w, -1.0);
    Frame {
        origin: tag.world([0.0, side * thickness / 2.0, middle]),
        u: cross(v, w),
        v,
        w,
    }
}

/// A tag leaning further than this from straight up counts as sticking out sideways (radians).
const SIDEWAYS: f64 = std::f64::consts::FRAC_PI_4;

fn bounds(mesh: &Mesh, tris: &[[u32; 3]]) -> (P3, P3) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for t in tris {
        for &i in t {
            let p = mesh.point(i);
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    (lo, hi)
}

/// The plate as a box `plate_w` × `thickness` × `plate_h` standing on the bridge's top at `GAP`
/// along the tag's w, with an open hole in its bottom at `hole` (world points, the socket's
/// loop, anticlockwise seen from the plate). The hole's points come first.
fn plate_mesh(
    tag: &Frame,
    hole: &[P3],
    plate_w: f64,
    plate_h: f64,
    thickness: f64,
) -> Option<Mesh> {
    let (x, y) = (plate_w / 2.0, thickness / 2.0);
    let (z0, z1) = (GAP, GAP + plate_h);
    let mut points: Vec<P3> = hole.to_vec();
    let corner = |points: &mut Vec<P3>, p: P3| {
        points.push(tag.world(p));
        points.len() - 1
    };
    let b = [[-x, -y], [x, -y], [x, y], [-x, y]].map(|[px, py]| corner(&mut points, [px, py, z0]));
    let t = [[-x, -y], [x, -y], [x, y], [-x, y]].map(|[px, py]| corner(&mut points, [px, py, z1]));
    let mut tris: Vec<[usize; 3]> = Vec::new();
    // Bottom: the rectangle round the hole, facing down the tag's w.
    let hole_flat: Vec<P2> = hole
        .iter()
        .rev()
        .map(|&p| {
            let q = tag.local(p);
            [q[0], q[1]]
        })
        .collect();
    let outer: Vec<P2> = [[-x, -y], [x, -y], [x, y], [-x, y]].to_vec();
    let bottom = Polygon::new(outer.clone(), vec![hole_flat.clone()]);
    if bottom.outer != outer || bottom.holes.first() != Some(&hole_flat) {
        return None;
    }
    let n = hole.len();
    // Polygon numbering: the outline's four corners, then the hole, which runs the socket's loop
    // backwards.
    let id = |k: usize| if k < 4 { b[k] } else { n - 1 - (k - 4) };
    for [p, q, r] in bottom.triangulate().ok()? {
        tris.push([id(p), id(r), id(q)]);
    }
    // Top, then the four sides, all facing out.
    tris.push([t[0], t[1], t[2]]);
    tris.push([t[0], t[2], t[3]]);
    for k in 0..4 {
        let k1 = (k + 1) % 4;
        tris.push([b[k], b[k1], t[k1]]);
        tris.push([b[k], t[k1], t[k]]);
    }
    Some(Mesh {
        positions: points.iter().flat_map(|p| p.map(|c| c as f32)).collect(),
        indices: tris.into_iter().flatten().map(|i| i as u32).collect(),
    })
}
