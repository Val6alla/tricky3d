//! Close trivial holes, Close all holes and Close defined holes: fill border loops with new
//! triangles that match the surrounding orientation. With a selection, Close trivial holes and
//! Close all holes only fill holes that border a selected triangle (Close defined holes always
//! needs one).
//!
//! Each hole is filled on its own, in parallel, and the patches are appended in loop order:
//! - A loop of 3 edges gets one triangle. For Close trivial holes, a loop of 4 edges gets two
//!   triangles over the diagonal whose triangles' normals are closer, when they are at most 20°
//!   apart; longer loops are not trivial.
//! - A flat loop (every corner within 2% of the loop's size from its plane, and the loop seen
//!   along the plane's normal a simple polygon) gets the constrained Delaunay triangulation of
//!   that polygon: a flat patch with no new vertices.
//! - Any other loop is triangulated over its own corners twice: following the surrounding
//!   surface, by Liepa's minimum-weight triangulation (the smallest largest dihedral angle, then
//!   the least area; a loop of more than 400 corners is first cut along short chords into pieces
//!   of at most 400), and, when the loop seen along its plane's normal is simple, flat in that
//!   plane. From the first, then the second, the patch is refined until its edges are about as
//!   long as the surrounding edges and faired: a least-squares bi-Laplacian with the ring fixed,
//!   which runs the patch on smoothly from the surrounding surface wherever that surface meets
//!   the patch within 60° (a hole in a curved surface), and leaves it free to meet the surface at
//!   an angle at a crease (a wall standing on the rim, a sharp lip, a surface folding back under
//!   the hole); failing that, it is faired as a membrane through the ring.
//! - A loop whose corners all lie exactly on one line (a slit, such as the crack along a
//!   T-junction) has no triangulation without zero-area triangles. It is zipped instead: each
//!   triangle along the slit is split at the corners of the other side that lie on its edge.
//!
//!
//! Close holes along the curvature (Home › Repair) fills every hole the way the last kind is
//! filled, flat loops and loops of 3 edges too: a hole in a curved surface whose rim happens to
//! lie in a plane (a cap cut off a sphere) gets a patch that carries the curve on, not a flat lid.
//! A small hole still gets the triangulation over its corners, since refinement adds no point
//! where the patch's triangles are already about as small as the surrounding ones.
//!
//! A patch never has a triangle of zero area, never joins two corners that a mesh edge already
//! joins (that edge would become non-manifold), and uses each border edge the opposite way to the
//! triangle beside it, so it continues that triangle's orientation. A faired patch that folds
//! (neighbouring triangles more than 90° apart) at more edges than the triangulation over the
//! ring's own corners, crosses itself, or bulges out further than the hole is wide gives way to
//! the next one; the last resort is that triangulation itself.

use std::f64::consts::PI;
use std::iter::once;
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;
use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};

use super::bvh::{Bvh, boxes_of};
use super::exact::{
    P2, P3, add, collinear, corners, cross, dot, length, normal, orient2d, scale, sub, vertex,
};
use super::topology::{BoundaryLoop, Edges, NONE, boundary_loops, side_ends};
use super::tritri::intersect;
use super::{Concern, Control, NEW, Origin, Outcome, RepairResult};
use crate::mesh::{Mesh, fmt_count};

/// Which holes a fill covers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Which<'a> {
    /// Bordered by 3 edges, or by 4 nearly planar edges.
    Trivial,
    All,
    /// Holes with a border edge on one of these triangles (sorted triangle indices).
    Touching(&'a [u32]),
}

/// A fill's result, for actions (such as Wrap part surface) that close holes on the way.
#[derive(Clone, Debug, PartialEq)]
pub struct Filled {
    pub mesh: Mesh,
    /// Input triangle of each output triangle, NEW for the patches (input triangles keep their
    /// order and come first).
    pub origin: Vec<u32>,
    pub closed: usize,
    pub added: usize,
    /// Holes that could not be filled.
    pub failed: usize,
}

/// How a fill treats the holes it covers (see the module doc).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// Only 3 edges, or 4 nearly flat ones (Close trivial holes).
    Trivial,
    /// Every hole; flat loops get flat patches (Close all holes, Close defined holes).
    All,
    /// Every hole, each refined and faired to carry on the surrounding surface (Close holes along
    /// the curvature).
    Curved,
}

/// The largest angle between the two triangles of a trivial 4-edge hole.
const TRIVIAL_ANGLE: f64 = 20.0 * PI / 180.0;
/// A loop is flat when no corner lies further than this share of its size from its plane.
const FLATNESS: f64 = 0.02;
/// Loops of up to this many corners get the minimum-weight triangulation, which takes n³ steps;
/// longer ones are cut into pieces of at most this many first.
const MWT_MAX: usize = 400;
/// The most corners tried as ends of each chord that cuts a long loop.
const CHORD_SAMPLES: usize = 1024;
/// Liepa's density factor: a patch triangle splits while its centroid lies further than σ / α
/// from each of its corners, σ being the surrounding edge length there.
const ALPHA: f64 = std::f64::consts::SQRT_2;
/// Refinement passes at most; each one shrinks the triangles' edges by about √3.
const PASSES: usize = 24;
/// The most triangles refinement grows one patch to; a bigger hole is refined more coarsely.
const PATCH_MAX: usize = 20_000;
/// Fairing iterations after each refinement pass.
const STEP_ITERATIONS: usize = 30;
/// Fairing stops early once the residual's gradient has shrunk by this factor.
const FAIR_TOLERANCE: f64 = 1e-7;
/// Rounds of untangling, edge relaxation and fairing for a refined patch that folds, and the
/// fairing iterations in each.
const SETTLE_ROUNDS: usize = 3;
const SETTLE_ITERATIONS: usize = 100;
/// Steps of moving the points round a fold to their neighbours' mean, per untangling.
const UNTANGLE_ROUNDS: usize = 10;

/// Fills the holes `which` names. The input triangles keep their order and come first; the
/// patches follow in loop order. Holes that cannot be filled stay open and count as `failed`.
pub fn fill(mesh: &Mesh, which: Which, control: &mut Control) -> RepairResult<Filled> {
    let (kind, only) = match which {
        Which::Trivial => (Kind::Trivial, None),
        Which::All => (Kind::All, None),
        Which::Touching(selection) => (Kind::All, Some(selection)),
    };
    Ok(close(mesh, kind, only, control)?.filled)
}

/// Fills holes bordered by 3 edges, or by 4 edges that are nearly flat. With a selection, only
/// those that border a selected triangle.
pub fn close_trivial(
    mesh: &Mesh,
    selection: &[u32],
    control: &mut Control,
) -> RepairResult<Outcome> {
    let only = (!selection.is_empty()).then_some(selection);
    let run = close(mesh, Kind::Trivial, only, control)?;
    Ok(report(run, true, only.is_some()))
}

/// Fills every hole: flat ones with flat patches, curved ones with patches that follow the
/// surface. With a selection, only those that border a selected triangle.
pub fn close_all(mesh: &Mesh, selection: &[u32], control: &mut Control) -> RepairResult<Outcome> {
    let only = (!selection.is_empty()).then_some(selection);
    let run = close(mesh, Kind::All, only, control)?;
    Ok(report(run, false, only.is_some()))
}

/// Fills every hole with a patch refined and faired to carry on the surrounding surface, flat
/// holes too. With a selection, only those that border a selected triangle.
pub fn close_curved(
    mesh: &Mesh,
    selection: &[u32],
    control: &mut Control,
) -> RepairResult<Outcome> {
    let only = (!selection.is_empty()).then_some(selection);
    let run = close(mesh, Kind::Curved, only, control)?;
    Ok(report(run, false, only.is_some()))
}

/// Fills the holes that border the selected triangles, whatever their size.
pub fn close_defined(
    mesh: &Mesh,
    selection: &[u32],
    control: &mut Control,
) -> RepairResult<Outcome> {
    control.check()?;
    if selection.is_empty() {
        return Ok(Outcome::unchanged(mesh, "No triangles are selected."));
    }
    let run = close(mesh, Kind::All, Some(selection), control)?;
    Ok(report(run, false, true))
}

/// A fill and how many border loops the mesh had, for the message.
struct Run {
    filled: Filled,
    holes: usize,
}

fn report(run: Run, trivial: bool, selection: bool) -> Outcome {
    let Run { filled, holes } = run;
    let mut warnings = Vec::new();
    let mut concerns = Vec::new();
    if filled.failed > 0 {
        warnings.push(format!(
            "{} could not be closed.",
            count(filled.failed, "hole", "holes")
        ));
        concerns.push((0, Concern::Holes));
    }
    if filled.closed == 0 {
        let message = if holes == 0 {
            "No holes to close."
        } else if filled.failed > 0 {
            "No holes were closed."
        } else if trivial && selection {
            "No trivial holes border the selected triangles."
        } else if trivial {
            "No trivial holes to close."
        } else {
            "No holes border the selected triangles."
        };
        return Outcome {
            mesh: filled.mesh,
            changed: false,
            origin: Origin::Same,
            message: message.into(),
            warnings,
            highlight: None,
            select: None,
            concerns,
        };
    }
    Outcome {
        message: format!(
            "Closed {}, added {}.",
            count(filled.closed, "hole", "holes"),
            count(filled.added, "triangle", "triangles")
        ),
        mesh: filled.mesh,
        changed: true,
        origin: Origin::Map(filled.origin),
        warnings,
        highlight: None,
        select: None,
        concerns,
    }
}

/// "1 hole", "1,204 holes".
fn count(n: usize, one: &str, many: &str) -> String {
    format!(
        "{} {}",
        fmt_count(n as u64),
        if n == 1 { one } else { many }
    )
}

/// Finds the border loops, keeps those wanted (only 3 and 4 edges for Kind::Trivial, only those
/// bordering `only` when given), fills each in parallel and appends the patches in loop order.
fn close(
    mesh: &Mesh,
    kind: Kind,
    only: Option<&[u32]>,
    control: &mut Control,
) -> RepairResult<Run> {
    control.check()?;
    let loops = {
        let edges = Edges::new(mesh);
        boundary_loops(mesh, &edges)
    };
    control.progress(0.1);
    let wanted: Vec<&BoundaryLoop> = loops
        .iter()
        .filter(|l| {
            (kind != Kind::Trivial || l.sides.len() <= 4)
                && only.is_none_or(|selection| {
                    l.sides
                        .iter()
                        .any(|&s| selection.binary_search(&(s / 3)).is_ok())
                })
        })
        .collect();
    if wanted.is_empty() {
        return Ok(Run {
            filled: untouched(mesh),
            holes: loops.len(),
        });
    }
    control.check()?;
    let around = Around::new(mesh, &wanted);
    control.progress(0.2);
    // Chunks between progress reports, each big enough to keep every thread busy.
    let cancel = control.cancel_flag();
    let per = wanted
        .len()
        .div_ceil(10)
        .max(4 * rayon::current_num_threads());
    let mut holes = Vec::with_capacity(wanted.len());
    for chunk in wanted.chunks(per) {
        let part: Vec<Hole> = chunk
            .par_iter()
            .map(|l| fill_hole(mesh, &around, l, kind, cancel))
            .collect();
        holes.extend(part);
        control.check()?;
        control.progress(0.2 + 0.7 * holes.len() as f64 / wanted.len() as f64);
    }
    let filled = assemble(mesh, holes);
    control.progress(1.0);
    Ok(Run {
        filled,
        holes: loops.len(),
    })
}

fn untouched(mesh: &Mesh) -> Filled {
    Filled {
        mesh: mesh.clone(),
        origin: (0..mesh.triangle_count() as u32).collect(),
        closed: 0,
        added: 0,
        failed: 0,
    }
}

// ---------------------------------------------------------------------------------------------
// One hole

/// What became of one hole.
enum Hole {
    /// Not trivial (Close trivial holes leaves it alone).
    Skipped,
    Failed,
    Closed(Piece),
}

/// The triangles that close one hole, in local vertex numbers: `vertices[i]` for the mesh
/// vertices they use, then the new points in order.
struct Piece {
    vertices: Vec<u32>,
    points: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    /// Per triangle: NEW, or the input triangle it is a piece of (a zipped slit splits triangles).
    origin: Vec<u32>,
    /// Input triangles a zipped slit replaces by their first piece.
    replaced: Vec<(u32, [u32; 3])>,
}

impl Piece {
    /// Triangles over the ring's own corners only.
    fn plain(ring: &Ring, triangles: Vec<[u32; 3]>) -> Piece {
        Piece {
            vertices: ring.vertices.clone(),
            points: Vec::new(),
            origin: vec![NEW; triangles.len()],
            triangles,
            replaced: Vec::new(),
        }
    }
}

/// Fills one hole: a slit is zipped, three corners get one triangle (unless the fill follows the
/// curvature), four flat corners two for Close trivial holes (which skips anything else), and any
/// other hole is left to `general`.
fn fill_hole(
    mesh: &Mesh,
    around: &Around,
    l: &BoundaryLoop,
    kind: Kind,
    cancel: &AtomicBool,
) -> Hole {
    if cancel.load(Ordering::Relaxed) {
        return Hole::Failed;
    }
    let ring = Ring::new(mesh, l);
    if on_one_line(&ring.points) {
        return zip(mesh, l).map_or(Hole::Failed, Hole::Closed);
    }
    let forbidden = Forbidden::new(mesh, around, &ring);
    match (ring.vertices.len(), kind) {
        // Three corners not on one line: the one triangle has area.
        (3, Kind::Trivial | Kind::All) => Hole::Closed(Piece::plain(&ring, vec![[0, 1, 2]])),
        (4, Kind::Trivial) => match quad(&ring, &forbidden) {
            Some(triangles) => Hole::Closed(Piece::plain(&ring, triangles)),
            None => Hole::Skipped,
        },
        (_, Kind::Trivial) => Hole::Skipped,
        _ => general(
            mesh,
            around,
            &ring,
            &forbidden,
            kind == Kind::Curved,
            cancel,
        ),
    }
}

/// A loop as the patch sees it.
struct Ring {
    /// The loop's mesh vertices in patch order: the border walk backwards, so the patch, going
    /// round its corners in this order, uses each border edge against the triangle beside it.
    vertices: Vec<u32>,
    points: Vec<P3>,
    /// Per patch edge i (corner i to corner i + 1): the unit normal of the mesh triangle across
    /// it, turned to agree with the patch's orientation; None when that triangle has no area.
    across: Vec<Option<P3>>,
}

impl Ring {
    fn new(mesh: &Mesh, l: &BoundaryLoop) -> Ring {
        let n = l.vertices.len();
        let vertices: Vec<u32> = (0..n).map(|i| l.vertices[(n - i) % n]).collect();
        let points = vertices.iter().map(|&v| vertex(mesh, v)).collect();
        // Patch edge i runs from vertices[(n − i) % n] to vertices[n − 1 − i] of the walk, along
        // the walk's side n − 1 − i the other way. A side that runs against the walk (where the
        // triangles round the hole disagree) belongs to a triangle facing the other way.
        let across = (0..n)
            .map(|i| {
                let j = n - 1 - i;
                let side = l.sides[j];
                let [a, b, c] = corners(mesh, side / 3);
                let forward = side_ends(mesh, side).0 == l.vertices[j];
                let n = normal(a, b, c);
                unit(n).map(|u| if forward { u } else { scale(u, -1.0) })
            })
            .collect();
        Ring {
            vertices,
            points,
            across,
        }
    }
}

/// Two triangles for a 4-edge hole: over the diagonal whose triangles' normals are closer (the
/// shorter diagonal when they are equally close, as on a flat hole), when they are at most
/// TRIVIAL_ANGLE apart. None when neither diagonal qualifies.
fn quad(ring: &Ring, forbidden: &Forbidden) -> Option<Vec<[u32; 3]>> {
    let p = &ring.points;
    let mut best: Option<(f64, f64, [[u32; 3]; 2])> = None;
    for (diagonal, pair) in [
        ((0, 2), [[0, 1, 2], [0, 2, 3]]),
        ((1, 3), [[0, 1, 3], [1, 2, 3]]),
    ] {
        if forbidden.contains(diagonal.0, diagonal.1) {
            continue;
        }
        let at = |t: [u32; 3]| t.map(|i| p[i as usize]);
        if pair.iter().any(|&t| {
            let [a, b, c] = at(t);
            collinear(a, b, c)
        }) {
            continue;
        }
        let [n1, n2] = pair.map(|t| {
            let [a, b, c] = at(t);
            normal(a, b, c)
        });
        let cos = dot(n1, n2) / (length(n1) * length(n2));
        let reach = dist2(p[diagonal.0], p[diagonal.1]);
        if best.is_none_or(|(c, r, _)| cos > c + 1e-12 || (cos >= c - 1e-12 && reach < r)) {
            best = Some((cos, reach, pair));
        }
    }
    let (cos, _, pair) = best?;
    (cos >= TRIVIAL_ANGLE.cos()).then(|| pair.to_vec())
}

/// Any hole of 4 or more corners, for Close all holes and Close defined holes; any hole at all
/// when `curved` (Close holes along the curvature), which refines and fairs flat loops too.
fn general(
    mesh: &Mesh,
    around: &Around,
    ring: &Ring,
    forbidden: &Forbidden,
    curved: bool,
    cancel: &AtomicBool,
) -> Hole {
    let n = ring.vertices.len();
    let shape = Shape::new(&ring.points);
    if !curved
        && shape.flat
        && let Some(polygon) = &shape.polygon
        && let Some(triangles) = cdt(polygon).and_then(|t| unforbidden(polygon, t, forbidden))
        && let Some(piece) =
            Patch::new(ring.points.clone(), triangles).and_then(|p| finish(ring, &p, forbidden))
    {
        return Hole::Closed(piece);
    }
    // Two triangulations over the ring's own corners: one that follows the surrounding surface
    // (the minimum-weight triangulation of the whole ring or, for a long ring, of pieces cut
    // along short chords), and the plane polygon's when that is simple. The plain patch is the
    // one that folds less, the first when they tie: where a jagged ring pinches, a triangulation
    // that follows the surface can fold where the flat one does not.
    let surface = if n <= MWT_MAX {
        mwt(
            &ring.points,
            &ring.across,
            |i, k| forbidden.contains(i, k),
            cancel,
        )
    } else {
        split_mwt(&ring.points, &ring.across, forbidden, cancel)
    }
    .and_then(|t| Patch::new(ring.points.clone(), t));
    let plane = shape
        .polygon
        .as_deref()
        .and_then(|polygon| cdt(polygon).and_then(|t| unforbidden(polygon, t, forbidden)))
        .and_then(|t| Patch::new(ring.points.clone(), t));
    let inner_folds = |p: &Patch| p.folds(&ring.across).map_or(usize::MAX, |f| f.inner);
    let plain = match (&surface, &plane) {
        (Some(surface), Some(plane)) if inner_folds(plane) < inner_folds(surface) => plane,
        (Some(surface), _) => surface,
        (None, Some(plane)) => plane,
        (None, None) => return Hole::Failed,
    };
    if !shape.flat || curved {
        // Refined and faired so it meets the surrounding surface smoothly where that surface
        // runs on into the hole; failing that (the patch would fold, or cross itself), as a
        // membrane that meets every ring corner at an angle. Started from the triangulation that
        // follows the surface, which keeps a big curved patch from sagging, and then from the
        // flat one, which keeps a patch over a jagged rim from pleating.
        let outer = Outer::new(mesh, around, ring);
        let membrane = vec![false; n];
        let inner = plain.folds(&ring.across).map_or(0, |f| f.inner);
        for start in [&surface, &plane].into_iter().flatten() {
            let smooth = start.smooth_corners(&ring.across);
            for mask in [&smooth, &membrane] {
                let mut patch = start.clone();
                if !patch.refine(&outer, mask, forbidden, cancel) {
                    break;
                }
                patch.settle(&outer, mask, forbidden, &ring.across);
                if patch.acceptable(&shape, &ring.across, inner)
                    && let Some(piece) = finish(ring, &patch, forbidden)
                    && !crosses_itself(mesh, &piece)
                {
                    return Hole::Closed(piece);
                }
                if !smooth.contains(&true) {
                    break;
                }
            }
        }
    }
    // The triangulation over the ring's corners, or the same with its edges relaxed: for a curved
    // ring whichever does not cross itself, then folds less (the plain one when they tie); for a
    // flat ring (one that is not simple in its plane) always relaxed, which only shapes the
    // triangles better.
    let mut relaxed = plain.clone();
    relaxed.relax(forbidden);
    let order = if shape.flat {
        [&relaxed, plain]
    } else {
        [plain, &relaxed]
    };
    let mut best: Option<(Piece, (bool, usize))> = None;
    for patch in order {
        let Some(piece) = finish(ring, patch, forbidden) else {
            continue;
        };
        let score = if shape.flat {
            (false, 0)
        } else {
            let folds = patch
                .folds(&ring.across)
                .map_or(usize::MAX, |f| f.inner + f.back);
            (crosses_itself(mesh, &piece), folds)
        };
        if best.as_ref().is_none_or(|(_, s)| score < *s) {
            best = Some((piece, score));
        }
    }
    best.map_or(Hole::Failed, |(piece, _)| Hole::Closed(piece))
}

/// The patch as a piece: new points rounded to f32, every triangle checked to have area after
/// the rounding, and no edge joining two ring corners that must not be joined (a last guard:
/// such an edge would give a mesh edge a third side). None when one fails.
fn finish(ring: &Ring, patch: &Patch, forbidden: &Forbidden) -> Option<Piece> {
    let fixed = patch.fixed;
    let points: Vec<[f32; 3]> = patch.points[fixed..]
        .iter()
        .map(|p| p.map(|x| x as f32))
        .collect();
    if points.iter().flatten().any(|x| !x.is_finite()) {
        return None;
    }
    let at = |i: u32| {
        let i = i as usize;
        if i < fixed {
            ring.points[i]
        } else {
            points[i - fixed].map(f64::from)
        }
    };
    let joins = |a: u32, b: u32| {
        (a as usize) < fixed
            && (b as usize) < fixed
            && !adjacent(a as usize, b as usize, fixed)
            && forbidden.contains(a as usize, b as usize)
    };
    if patch.tris.iter().any(|t| {
        collinear(at(t[0]), at(t[1]), at(t[2]))
            || joins(t[0], t[1])
            || joins(t[1], t[2])
            || joins(t[2], t[0])
    }) {
        return None;
    }
    Some(Piece {
        vertices: ring.vertices.clone(),
        points,
        triangles: patch.tris.clone(),
        origin: vec![NEW; patch.tris.len()],
        replaced: Vec::new(),
    })
}

/// Whether ring corners `i` and `k` are neighbours on a ring of `n`.
fn adjacent(i: usize, k: usize, n: usize) -> bool {
    (i + 1) % n == k || (k + 1) % n == i
}

/// True when every point lies exactly on one line (or all in one place).
fn on_one_line(points: &[P3]) -> bool {
    let a = points[0];
    let Some(b) = points
        .iter()
        .copied()
        .max_by(|p, q| dist2(a, *p).total_cmp(&dist2(a, *q)))
    else {
        return true;
    };
    a == b || points.iter().all(|&p| collinear(a, b, p))
}

// ---------------------------------------------------------------------------------------------
// What the patch needs from round the hole

/// The triangles round each corner of the holes being filled, and on how many of those holes each
/// corner lies. One pass over the triangles, so only the corners' own lists are kept.
struct Around {
    /// Per mesh vertex: its row, or NONE when it is on no hole.
    slot: Vec<u32>,
    start: Vec<u32>,
    triangles: Vec<u32>,
    holes: Vec<u32>,
}

impl Around {
    fn new(mesh: &Mesh, loops: &[&BoundaryLoop]) -> Around {
        let mut slot = vec![NONE; mesh.vertex_count()];
        let mut holes: Vec<u32> = Vec::new();
        for l in loops {
            for &v in &l.vertices {
                let s = &mut slot[v as usize];
                if *s == NONE {
                    *s = holes.len() as u32;
                    holes.push(0);
                }
                holes[*s as usize] += 1;
            }
        }
        let mut start = vec![0u32; holes.len() + 1];
        for &v in &mesh.indices {
            let s = slot[v as usize];
            if s != NONE {
                start[s as usize + 1] += 1;
            }
        }
        for s in 0..holes.len() {
            start[s + 1] += start[s];
        }
        let mut fill = start[..holes.len()].to_vec();
        let mut triangles = vec![0u32; start[holes.len()] as usize];
        for (t, c) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
            for &v in c {
                let s = slot[v as usize];
                if s != NONE {
                    triangles[fill[s as usize] as usize] = t as u32;
                    fill[s as usize] += 1;
                }
            }
        }
        Around {
            slot,
            start,
            triangles,
            holes,
        }
    }

    /// The triangles that use vertex `v` (a hole's corner).
    fn triangles(&self, v: u32) -> &[u32] {
        let s = self.slot[v as usize] as usize;
        &self.triangles[self.start[s] as usize..self.start[s + 1] as usize]
    }

    /// On how many of the holes being filled vertex `v` lies.
    fn holes(&self, v: u32) -> u32 {
        self.holes[self.slot[v as usize] as usize]
    }
}

/// Whether two of a piece's triangles cross, decided exactly on the coordinates the mesh will
/// have (the new points rounded to f32). Neighbours that only share a corner or an edge do not
/// cross; a patch folded flat onto itself does. (Crossing the mesh round the hole is not asked:
/// where that surface curls into the hole or another shell runs through it, every patch does,
/// and Wrap part surface resolves it.)
fn crosses_itself(mesh: &Mesh, piece: &Piece) -> bool {
    let points: Vec<P3> = piece
        .vertices
        .iter()
        .map(|&v| vertex(mesh, v))
        .chain(piece.points.iter().map(|p| p.map(f64::from)))
        .collect();
    let triangles: Vec<[P3; 3]> = piece
        .triangles
        .iter()
        .map(|t| t.map(|i| points[i as usize]))
        .collect();
    let mut crossed = false;
    Bvh::new(&boxes_of(&points, &piece.triangles)).self_pairs(|i, j| {
        crossed = crossed || intersect(&triangles[i as usize], &triangles[j as usize]);
    });
    crossed
}

/// Pairs of ring corners a patch edge must not join: corners a mesh edge already joins (other
/// than the ring's own edges), whose edge would get a third side; and corners that both lie on
/// another hole being filled too, whose patch might join them as well.
struct Forbidden {
    /// (lower, higher) ring positions, sorted.
    pairs: Vec<(u32, u32)>,
}

impl Forbidden {
    fn new(mesh: &Mesh, around: &Around, ring: &Ring) -> Forbidden {
        let n = ring.vertices.len();
        let mut place: Vec<(u32, u32)> = ring
            .vertices
            .iter()
            .enumerate()
            .map(|(i, &v)| (v, i as u32))
            .collect();
        place.sort_unstable();
        let find = |v: u32| {
            place
                .binary_search_by_key(&v, |&(w, _)| w)
                .ok()
                .map(|k| place[k].1 as usize)
        };
        let mut pairs = Vec::new();
        let mut shared = Vec::new();
        for (i, &v) in ring.vertices.iter().enumerate() {
            if around.holes(v) > 1 {
                shared.push(i);
            }
            for &t in around.triangles(v) {
                for &w in &mesh.indices[t as usize * 3..t as usize * 3 + 3] {
                    if w != v
                        && let Some(k) = find(w)
                        && !adjacent(i, k, n)
                    {
                        pairs.push((i.min(k) as u32, i.max(k) as u32));
                    }
                }
            }
        }
        for (x, &a) in shared.iter().enumerate() {
            for &b in &shared[x + 1..] {
                if !adjacent(a, b, n) {
                    pairs.push((a.min(b) as u32, a.max(b) as u32));
                }
            }
        }
        pairs.sort_unstable();
        pairs.dedup();
        Forbidden { pairs }
    }

    fn contains(&self, i: usize, k: usize) -> bool {
        !self.pairs.is_empty()
            && self
                .pairs
                .binary_search(&(i.min(k) as u32, i.max(k) as u32))
                .is_ok()
    }
}

/// Per ring corner, what fairing and refinement need from the mesh round it: the mean length of
/// its mesh edges (σ, the patch's edge length there), and its mesh neighbours as offsets
/// resampled at that length, summed and counted (the fixed part of its umbrella).
///
/// Resampling keeps the umbrella from being dominated by whichever neighbours happen to be close:
/// on a mesh of long thin cells, a corner's near neighbours would pull the patch's points to
/// the same short distance and squeeze its triangles into slivers. Each offset is split into its
/// part in the corner's tangent plane and its part along the normal; the tangent part is scaled
/// to length σ, and the normal part by the square of the same factor, so the offset still
/// describes how the surface bends that way (a circle's sagitta grows with the square of the
/// distance).
struct Outer {
    sigma: Vec<f64>,
    offsets: Vec<P3>,
    count: Vec<f64>,
}

impl Outer {
    fn new(mesh: &Mesh, around: &Around, ring: &Ring) -> Outer {
        let n = ring.vertices.len();
        let mut outer = Outer {
            sigma: Vec::with_capacity(n),
            offsets: Vec::with_capacity(n),
            count: Vec::with_capacity(n),
        };
        let mut neighbours = Vec::new();
        for (&v, &p) in ring.vertices.iter().zip(&ring.points) {
            neighbours.clear();
            let mut normal_sum = [0.0; 3];
            for &t in around.triangles(v) {
                let c = &mesh.indices[t as usize * 3..t as usize * 3 + 3];
                neighbours.extend(c.iter().filter(|&&w| w != v));
                let [a, b, d] = corners(mesh, t);
                normal_sum = add(normal_sum, normal(a, b, d));
            }
            neighbours.sort_unstable();
            neighbours.dedup();
            let lengths: f64 = neighbours
                .iter()
                .map(|&w| length(sub(vertex(mesh, w), p)))
                .sum();
            let sigma = lengths / neighbours.len().max(1) as f64;
            let up = unit(normal_sum).unwrap_or([0.0; 3]);
            let mut offsets = [0.0; 3];
            for &w in &neighbours {
                let d = sub(vertex(mesh, w), p);
                let rise = dot(d, up);
                let flat = sub(d, scale(up, rise));
                let reach = length(flat);
                let (k, lift) = if reach > 0.25 * length(d) {
                    let k = sigma / reach;
                    (k, (rise * k * k).clamp(-sigma, sigma))
                } else {
                    // Nearly straight up or down from the corner (a crease): keep the direction.
                    let k = sigma / length(d).max(1e-300);
                    (k, rise * k)
                };
                offsets = add(offsets, add(scale(flat, k), scale(up, lift)));
            }
            outer.sigma.push(sigma);
            outer.offsets.push(offsets);
            outer.count.push(neighbours.len() as f64);
        }
        outer
    }
}

/// The loop's plane and how the loop lies in it.
struct Shape {
    center: P3,
    /// Unit normal of the loop's plane (Newell's normal), None when the loop encloses no area.
    normal: Option<P3>,
    /// The loop's diameter: the largest distance between two corners.
    size: f64,
    /// Every corner within FLATNESS × size of the plane.
    flat: bool,
    /// The loop in the plane's coordinates when it is a simple counter-clockwise polygon there.
    polygon: Option<Vec<P2>>,
}

impl Shape {
    fn new(points: &[P3]) -> Shape {
        let n = points.len();
        let center = scale(
            points.iter().fold([0.0; 3], |s, &p| add(s, p)),
            1.0 / n as f64,
        );
        let size = diameter(points);
        // Twice the vector area, as a fan from the first corner (Newell's normal).
        let o = points[0];
        let mut area = [0.0; 3];
        for i in 1..n - 1 {
            area = add(area, cross(sub(points[i], o), sub(points[i + 1], o)));
        }
        let mut shape = Shape {
            center,
            normal: None,
            size,
            flat: false,
            polygon: None,
        };
        let len = length(area);
        if len.is_nan() || len <= 1e-12 * size * size {
            return shape;
        }
        let normal = scale(area, 1.0 / len);
        let deviation = points
            .iter()
            .map(|&p| dot(sub(p, center), normal).abs())
            .fold(0.0, f64::max);
        shape.flat = deviation <= FLATNESS * size;
        let (u, w) = basis(normal);
        let polygon: Vec<P2> = points
            .iter()
            .map(|&p| {
                let d = sub(p, center);
                [dot(d, u), dot(d, w)]
            })
            .collect();
        if signed_area(&polygon) > 0.0 && is_simple(&polygon) {
            shape.polygon = Some(polygon);
        }
        shape.normal = Some(normal);
        shape
    }

    /// How far `p` lies from the plane (from the centre when there is no plane).
    fn distance(&self, p: P3) -> f64 {
        let d = sub(p, self.center);
        match self.normal {
            Some(n) => dot(d, n).abs(),
            None => length(d),
        }
    }
}

/// The largest distance between two points: exact for up to 128 points, else from two sweeps
/// for the farthest point (the farthest from the first point, then the farthest from that one),
/// which is never more than the diameter and rarely much less.
fn diameter(points: &[P3]) -> f64 {
    let farthest = |from: P3| {
        points
            .iter()
            .copied()
            .max_by(|p, q| dist2(from, *p).total_cmp(&dist2(from, *q)))
            .unwrap_or(from)
    };
    if points.len() > 128 {
        let a = farthest(points[0]);
        return length(sub(farthest(a), a));
    }
    let mut most = 0.0f64;
    for (i, &p) in points.iter().enumerate() {
        for &q in &points[i + 1..] {
            most = most.max(dist2(p, q));
        }
    }
    most.sqrt()
}

/// Two unit vectors that make a right-handed frame with the unit vector `n`, so a polygon that
/// turns counter-clockwise round `n` turns counter-clockwise in their coordinates.
fn basis(n: P3) -> (P3, P3) {
    let k = (0..3)
        .min_by(|&i, &j| n[i].abs().total_cmp(&n[j].abs()))
        .unwrap_or(0);
    let mut e = [0.0; 3];
    e[k] = 1.0;
    let u = cross(n, e);
    let u = scale(u, 1.0 / length(u));
    (u, cross(n, u))
}

/// Twice the signed area of a polygon (positive counter-clockwise).
fn signed_area(q: &[P2]) -> f64 {
    let o = q[0];
    (1..q.len() - 1)
        .map(|i| {
            let (a, b) = (q[i], q[i + 1]);
            (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
        })
        .sum()
}

/// Whether a closed polygon is simple, decided exactly on its coordinates: no edge of zero
/// length, no corner where the edges fold back over each other, and no two edges that are not
/// neighbours touching at all. A sweep over x, so a well-shaped polygon takes about n log n.
fn is_simple(q: &[P2]) -> bool {
    let n = q.len();
    for i in 0..n {
        let (a, b, c) = (q[(i + n - 1) % n], q[i], q[(i + 1) % n]);
        if b == c || (orient2d(a, b, c) == 0 && same_way(a, b, c)) {
            return false;
        }
    }
    if n == 3 {
        return true;
    }
    let low = |e: usize| q[e][0].min(q[(e + 1) % n][0]);
    let high = |e: usize| q[e][0].max(q[(e + 1) % n][0]);
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_unstable_by(|&a, &b| low(a).total_cmp(&low(b)).then(a.cmp(&b)));
    for (x, &e) in order.iter().enumerate() {
        let top = high(e);
        for &f in &order[x + 1..] {
            if low(f) > top {
                break;
            }
            if !adjacent(e, f, n) && touch(q[e], q[(e + 1) % n], q[f], q[(f + 1) % n]) {
                return false;
            }
        }
    }
    true
}

/// For `a`, `b`, `c` on one line: true when `a` and `c` lie on the same side of `b`.
fn same_way(a: P2, b: P2, c: P2) -> bool {
    let sign = |x: f64| (x > 0.0) as i32 - (x < 0.0) as i32;
    sign(a[0] - b[0]) == sign(c[0] - b[0]) && sign(a[1] - b[1]) == sign(c[1] - b[1])
}

/// Whether the closed segments a-b and c-d share a point. Exact.
fn touch(a: P2, b: P2, c: P2, d: P2) -> bool {
    let (o1, o2) = (orient2d(a, b, c), orient2d(a, b, d));
    if o1 * o2 > 0 {
        return false;
    }
    let (o3, o4) = (orient2d(c, d, a), orient2d(c, d, b));
    if o3 * o4 > 0 {
        return false;
    }
    if o1 == 0 && o2 == 0 {
        // All four on one line: they touch when their extents overlap.
        let overlap =
            |k: usize| a[k].min(b[k]).max(c[k].min(d[k])) <= a[k].max(b[k]).min(c[k].max(d[k]));
        return overlap(0) && overlap(1);
    }
    true
}

// ---------------------------------------------------------------------------------------------
// Triangulations of a ring (triangles in ring positions, in patch orientation)

/// The constrained Delaunay triangulation of a simple counter-clockwise polygon: n − 2 triangles
/// over its own corners. None when the triangulator disagrees that the polygon is simple.
fn cdt(polygon: &[P2]) -> Option<Vec<[u32; 3]>> {
    let n = polygon.len();
    let points: Vec<Point2<f64>> = polygon.iter().map(|p| Point2::new(p[0], p[1])).collect();
    let edges: Vec<[usize; 2]> = (0..n).map(|i| [i, (i + 1) % n]).collect();
    let mut conflict = false;
    let cdt: ConstrainedDelaunayTriangulation<Point2<f64>> =
        ConstrainedDelaunayTriangulation::try_bulk_load_cdt(points, edges, |_| conflict = true)
            .ok()?;
    if conflict || cdt.num_vertices() != n {
        return None;
    }
    // Every face lies wholly inside the polygon or wholly outside. Inside, its corners follow the
    // polygon's order round it (counter-clockwise, like every face); outside, the reverse.
    let mut out = Vec::with_capacity(n - 2);
    for face in cdt.inner_faces() {
        let v = face.vertices().map(|h| h.fix().index() as u32);
        let r = (0..3).min_by_key(|&k| v[k]).unwrap_or(0);
        let t = [v[r], v[(r + 1) % 3], v[(r + 2) % 3]];
        if t[1] < t[2] {
            out.push(t);
        }
    }
    (out.len() == n - 2).then_some(out)
}

/// A triangulation of the plane polygon with every chord that `forbidden` rules out flipped to
/// the other diagonal of its two triangles, where that diagonal is allowed and the two triangles
/// make a convex quad in the plane. (Seen along the hole's normal, the surface beside a hole can
/// cover part of it, so the plane's Delaunay chord may be a mesh edge, such as the base of a
/// triangle sticking out of a jagged rim.) Each flip replaces a forbidden chord with an allowed
/// one, so the passes end; a flip renumbers its triangles' sides, so only a pass without flips
/// has seen every chord. None when a forbidden chord cannot be flipped away.
fn unforbidden(
    polygon: &[P2],
    triangles: Vec<[u32; 3]>,
    forbidden: &Forbidden,
) -> Option<Vec<[u32; 3]>> {
    if forbidden.pairs.is_empty() {
        return Some(triangles);
    }
    let mut patch = Patch::new(
        polygon.iter().map(|p| [p[0], p[1], 0.0]).collect(),
        triangles,
    )?;
    let at = |v: u32| polygon[v as usize];
    loop {
        let (mut flipped, mut left) = (false, false);
        for s in 0..patch.twin.len() as u32 {
            let r = patch.twin[s as usize];
            let (a, b) = (patch.start(s), patch.end(s));
            if r == NONE || r < s || !forbidden.contains(a as usize, b as usize) {
                continue;
            }
            let (c, d) = (patch.start(prev(s)), patch.start(prev(r)));
            if orient2d(at(a), at(d), at(c)) > 0
                && orient2d(at(d), at(b), at(c)) > 0
                && !forbidden.contains(c as usize, d as usize)
            {
                patch.flip(s);
                flipped = true;
            } else {
                left = true;
            }
        }
        if !flipped {
            return (!left).then_some(patch.tris);
        }
    }
}

/// One cell of the minimum-weight triangulation's table: the best triangulation of the
/// sub-polygon from corner i to corner k (closed by the chord k → i).
#[derive(Clone, Copy)]
struct Cell {
    /// Cosine of its largest dihedral angle (2 for a lone edge, −∞ when there is none).
    worst: f32,
    /// Unit normal of the triangle across the chord from the next triangle, and in the last
    /// place: 0 for a patch triangle, −1 for the mesh triangle across a ring edge (whose crease
    /// counts as no sharper than a right angle), 1 where there is no triangle to compare with (a
    /// ring edge whose mesh triangle has no area).
    facing: [f32; 4],
    area: f64,
}

const NO_CELL: Cell = Cell {
    worst: f32::NEG_INFINITY,
    facing: [0.0; 4],
    area: 0.0,
};

/// Liepa's minimum-weight triangulation (after Barequet and Sharir): of all triangulations over
/// the ring's own corners, the one whose largest dihedral angle (between neighbouring patch
/// triangles, and between a patch triangle and the mesh triangle across a ring edge, that one
/// counted as a right angle at most) is smallest, then the one with the least area. `forbid(i, k)` rules out the chord between corners i and k;
/// triangles with no area are never used. Dynamic programming over sub-polygons: O(n³) time,
/// O(n²) memory. A first pass also keeps out slivers so thin that their normals are noise; the
/// second allows them. None when no triangulation qualifies or on cancel.
fn mwt(
    p: &[P3],
    across: &[Option<P3>],
    forbid: impl Fn(usize, usize) -> bool,
    cancel: &AtomicBool,
) -> Option<Vec<[u32; 3]>> {
    if p.len() < 3 {
        return None;
    }
    mwt_pass(p, across, &forbid, true, cancel)
        .or_else(|| mwt_pass(p, across, &forbid, false, cancel))
}

fn mwt_pass(
    p: &[P3],
    across: &[Option<P3>],
    forbid: &dyn Fn(usize, usize) -> bool,
    strict: bool,
    cancel: &AtomicBool,
) -> Option<Vec<[u32; 3]>> {
    let n = p.len();
    // Each cell twice, by row (i, ·) and by column (·, k), so the inner loop reads both runs of
    // cells it combines from consecutive memory.
    let mut by_row = vec![NO_CELL; n * n];
    let mut by_column = vec![NO_CELL; n * n];
    let mut split = vec![0u16; n * n];
    let facing = |a: Option<P3>| match a {
        Some(v) => [v[0] as f32, v[1] as f32, v[2] as f32, -1.0],
        None => [0.0, 0.0, 0.0, 1.0],
    };
    for i in 0..n - 1 {
        let edge = Cell {
            worst: 2.0,
            facing: facing(across[i]),
            area: 0.0,
        };
        by_row[i * n + i + 1] = edge;
        by_column[(i + 1) * n + i] = edge;
    }
    let closing = facing(across[n - 1]);
    // A crease sharper than a right angle with the mesh beside the hole is a crease of the rim
    // (a lip, or the edge of a cut), which bending the patch's triangles to soften would only
    // fold them: it counts as a right angle.
    let cosine = |t: P3, f: [f32; 4]| {
        let c = t[0] * f[0] as f64 + t[1] * f[1] as f64 + t[2] * f[2] as f64;
        if f[3] < 0.0 {
            c.max(0.0)
        } else {
            c + f[3] as f64
        }
    };
    for gap in 2..n {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        for i in 0..n - gap {
            let k = i + gap;
            // The whole ring's closing edge is a ring edge, never a chord.
            if gap < n - 1 && forbid(i, k) {
                continue;
            }
            let (pi, pk) = (p[i], p[k]);
            let ik = sub(pk, pi);
            let ik2 = dot(ik, ik);
            let row = &by_row[i * n..i * n + n];
            let column = &by_column[k * n..k * n + n];
            let mut best = NO_CELL;
            let mut best_m = 0;
            for m in i + 1..k {
                let (left, right) = (row[m], column[m]);
                if left.worst == f32::NEG_INFINITY || right.worst == f32::NEG_INFINITY {
                    continue;
                }
                // The new triangle can only lower the parts' worst cosine and add area, so a
                // split whose parts already lose to the best one is passed over unmeasured.
                if best.worst != f32::NEG_INFINITY {
                    let (bound, b) = (left.worst.min(right.worst) as f64, best.worst as f64);
                    if bound < b - 1e-6
                        || (bound <= b + 1e-6 && left.area + right.area >= best.area)
                    {
                        continue;
                    }
                }
                let im = sub(p[m], pi);
                let c = cross(im, ik);
                let c2 = dot(c, c);
                if c2 == 0.0
                    || (c2 <= 1e-20 * dot(im, im) * ik2 && (strict || collinear(pi, p[m], pk)))
                {
                    continue;
                }
                let len = c2.sqrt();
                let t = scale(c, 1.0 / len);
                let mut worst = (left.worst.min(right.worst) as f64)
                    .min(cosine(t, left.facing))
                    .min(cosine(t, right.facing));
                if gap == n - 1 {
                    worst = worst.min(cosine(t, closing));
                }
                let area = left.area + right.area + 0.5 * len;
                if better(worst, area, &best) {
                    best = Cell {
                        worst: worst as f32,
                        facing: [t[0] as f32, t[1] as f32, t[2] as f32, 0.0],
                        area,
                    };
                    best_m = m;
                }
            }
            if best.worst != f32::NEG_INFINITY {
                by_row[i * n + k] = best;
                by_column[k * n + i] = best;
                split[i * n + k] = best_m as u16;
            }
        }
    }
    if by_row[n - 1].worst == f32::NEG_INFINITY {
        return None;
    }
    let mut out = Vec::with_capacity(n - 2);
    let mut stack = vec![(0, n - 1)];
    while let Some((i, k)) = stack.pop() {
        if k - i < 2 {
            continue;
        }
        let m = split[i * n + k] as usize;
        out.push([i as u32, m as u32, k as u32]);
        stack.push((i, m));
        stack.push((m, k));
    }
    Some(out)
}

/// Whether (worst, area) beats `best`: a clearly smaller largest angle, or about the same and less
/// area. Cosines within 1e-6 count as the same angle, since rounding scatters equal ones.
fn better(worst: f64, area: f64, best: &Cell) -> bool {
    if best.worst == f32::NEG_INFINITY {
        return true;
    }
    let b = best.worst as f64;
    if worst > b + 1e-6 {
        true
    } else if worst < b - 1e-6 {
        false
    } else {
        area < best.area
    }
}

/// Triangulates a ring too long for one minimum-weight triangulation: cuts it in two along the
/// chord between the corners that are closest compared with how far apart they lie along the
/// ring (each part keeping at least a quarter of the corners), again until every part has at
/// most MWT_MAX corners, then triangulates the parts in parallel. A chord has no mesh triangle
/// across it.
fn split_mwt(
    points: &[P3],
    across: &[Option<P3>],
    forbidden: &Forbidden,
    cancel: &AtomicBool,
) -> Option<Vec<[u32; 3]>> {
    let n = points.len();
    let mut parts: Vec<Vec<u32>> = Vec::new();
    let mut stack = vec![(0..n as u32).collect::<Vec<u32>>()];
    while let Some(part) = stack.pop() {
        if part.len() <= MWT_MAX {
            parts.push(part);
            continue;
        }
        let (a, b) = chord(points, &part, forbidden)?;
        let first = part[a..=b].to_vec();
        let second: Vec<u32> = part[b..].iter().chain(&part[..=a]).copied().collect();
        stack.push(first);
        stack.push(second);
    }
    let triangulated: Vec<Option<Vec<[u32; 3]>>> = parts
        .par_iter()
        .map(|part| {
            let m = part.len();
            let p: Vec<P3> = part.iter().map(|&i| points[i as usize]).collect();
            let a: Vec<Option<P3>> = (0..m)
                .map(|i| {
                    let (u, v) = (part[i] as usize, part[(i + 1) % m] as usize);
                    if v == (u + 1) % n { across[u] } else { None }
                })
                .collect();
            let triangles = mwt(
                &p,
                &a,
                |i, k| forbidden.contains(part[i] as usize, part[k] as usize),
                cancel,
            )?;
            Some(
                triangles
                    .into_iter()
                    .map(|t| t.map(|i| part[i as usize]))
                    .collect(),
            )
        })
        .collect();
    let mut out = Vec::with_capacity(n - 2);
    for t in triangulated {
        out.extend(t?);
    }
    Some(out)
}

/// The chord to cut a long ring part along: positions a < b in `part`, at least a quarter of the
/// part apart either way round, minimising the chord's length over the shorter way round. On a
/// part of more than CHORD_SAMPLES corners only every few corners are tried as ends (all of
/// them when none of those may be joined), so a ring of any length costs at most about
/// CHORD_SAMPLES² steps per cut.
fn chord(points: &[P3], part: &[u32], forbidden: &Forbidden) -> Option<(usize, usize)> {
    let m = part.len();
    let at = |i: usize| points[part[i] as usize];
    let mut along = vec![0.0; m + 1];
    for i in 0..m {
        along[i + 1] = along[i] + length(sub(at((i + 1) % m), at(i)));
    }
    let total = along[m];
    let quarter = (m / 4).max(2);
    let search = |stride: usize| {
        let mut best: Option<(f64, usize, usize)> = None;
        for a in (0..m).step_by(stride) {
            for b in (a + quarter..m).step_by(stride) {
                if m - (b - a) < quarter {
                    break;
                }
                if forbidden.contains(part[a] as usize, part[b] as usize) {
                    continue;
                }
                let way = along[b] - along[a];
                let shorter = way.min(total - way);
                if shorter <= 0.0 {
                    continue;
                }
                let ratio = length(sub(at(b), at(a))) / shorter;
                if best.is_none_or(|(r, _, _)| ratio < r) {
                    best = Some((ratio, a, b));
                }
            }
        }
        best.map(|(_, a, b)| (a, b))
    };
    let stride = m.div_ceil(CHORD_SAMPLES);
    search(stride).or_else(|| if stride > 1 { search(1) } else { None })
}

/// Zips a slit: a loop whose corners all lie exactly on one line. Each border side's triangle is
/// split at the loop's corners that lie strictly inside that side, so both sides of the slit end
/// up with the same pieces of the line, each used once from either side. None when two corners
/// share a place, a border triangle has no area, or the pieces do not pair up.
fn zip(mesh: &Mesh, l: &BoundaryLoop) -> Option<Piece> {
    let at = |v: u32| vertex(mesh, v);
    let p0 = at(l.vertices[0]);
    let far = l
        .vertices
        .iter()
        .map(|&v| at(v))
        .max_by(|a, b| dist2(p0, *a).total_cmp(&dist2(p0, *b)))?;
    let d = sub(far, p0);
    // Along the axis the line runs along most, the corners' order is exact.
    let axis = (0..3).max_by(|&i, &j| d[i].abs().total_cmp(&d[j].abs()))?;
    if d[axis] == 0.0 {
        return None;
    }
    let place = |v: u32| at(v)[axis] * d[axis].signum();
    let mut line: Vec<(f64, u32)> = l.vertices.iter().map(|&v| (place(v), v)).collect();
    line.sort_by(|a, b| a.0.total_cmp(&b.0));
    if line.windows(2).any(|w| w[0].0 == w[1].0) {
        return None;
    }
    fn local(vertices: &mut Vec<u32>, v: u32) -> u32 {
        match vertices.iter().position(|&w| w == v) {
            Some(i) => i as u32,
            None => {
                vertices.push(v);
                (vertices.len() - 1) as u32
            }
        }
    }
    let mut vertices = l.vertices.clone();
    let mut triangles = Vec::new();
    let mut origin = Vec::new();
    let mut replaced: Vec<(u32, [u32; 3])> = Vec::new();
    let mut pieces: Vec<(u32, u32)> = Vec::new();
    for &side in &l.sides {
        let t = side / 3;
        let k = (side % 3) as usize;
        let c = &mesh.indices[t as usize * 3..t as usize * 3 + 3];
        let (u, v, x) = (c[k], c[(k + 1) % 3], c[(k + 2) % 3]);
        if collinear(at(u), at(v), at(x)) || replaced.iter().any(|r| r.0 == t) {
            return None;
        }
        let (pu, pv) = (place(u), place(v));
        let (lo, hi) = (pu.min(pv), pu.max(pv));
        let mut inside: Vec<u32> = line
            .iter()
            .filter(|&&(q, _)| q > lo && q < hi)
            .map(|&(_, w)| w)
            .collect();
        if pu > pv {
            inside.reverse();
        }
        let chain: Vec<u32> = once(u)
            .chain(inside.iter().copied())
            .chain(once(v))
            .collect();
        pieces.extend(chain.windows(2).map(|w| (w[0].min(w[1]), w[0].max(w[1]))));
        if inside.is_empty() {
            continue;
        }
        // The triangle (u, v, x) becomes the fan (u, w1, x), (w1, w2, x), …, (wk, v, x), each
        // facing the way it did.
        let apex = local(&mut vertices, x);
        for (i, w) in chain.windows(2).enumerate() {
            let piece = [local(&mut vertices, w[0]), local(&mut vertices, w[1]), apex];
            if i == 0 {
                replaced.push((t, piece));
            } else {
                triangles.push(piece);
                origin.push(t);
            }
        }
    }
    pieces.sort_unstable();
    let paired = pieces.len().is_multiple_of(2)
        && pieces.chunks(2).all(|w| w[0] == w[1])
        && pieces.windows(3).all(|w| !(w[0] == w[1] && w[1] == w[2]));
    if !paired || replaced.is_empty() {
        return None;
    }
    Some(Piece {
        vertices,
        points: Vec::new(),
        triangles,
        origin,
        replaced,
    })
}

// ---------------------------------------------------------------------------------------------
// A patch being refined

/// A triangulated patch with side adjacency, so it can be split, flipped and faired.
#[derive(Clone)]
struct Patch {
    /// The ring's corners (first, fixed) and the points refinement added.
    points: Vec<P3>,
    /// The edge length wanted round each point.
    sigma: Vec<f64>,
    tris: Vec<[u32; 3]>,
    /// The other side of each triangle side `3t + k` (corner k to corner k + 1), NONE on the
    /// ring's edges.
    twin: Vec<u32>,
    /// A side starting at each point.
    corner: Vec<u32>,
    /// Points before this one are the ring's corners.
    fixed: usize,
}

fn next(s: u32) -> u32 {
    s / 3 * 3 + (s % 3 + 1) % 3
}

fn prev(s: u32) -> u32 {
    s / 3 * 3 + (s % 3 + 2) % 3
}

impl Patch {
    /// A patch over the ring's points. None unless every ring edge has exactly one side, running
    /// the patch's way, and every other edge two sides running opposite ways.
    fn new(points: Vec<P3>, tris: Vec<[u32; 3]>) -> Option<Patch> {
        let fixed = points.len();
        let mut keys: Vec<(u32, u32, u32)> = Vec::with_capacity(tris.len() * 3);
        for (t, c) in tris.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (c[k], c[(k + 1) % 3]);
                if a == b || a as usize >= fixed || b as usize >= fixed {
                    return None;
                }
                keys.push((a.min(b), a.max(b), (3 * t + k) as u32));
            }
        }
        keys.sort_unstable();
        let mut twin = vec![NONE; keys.len()];
        let mut open = 0;
        let mut i = 0;
        while i < keys.len() {
            let mut j = i + 1;
            while j < keys.len() && (keys[j].0, keys[j].1) == (keys[i].0, keys[i].1) {
                j += 1;
            }
            let start = |s: u32| tris[s as usize / 3][s as usize % 3];
            match j - i {
                1 => {
                    let s = keys[i].2;
                    let (a, b) = (start(s), start(next(s)));
                    if b as usize != (a as usize + 1) % fixed {
                        return None;
                    }
                    open += 1;
                }
                2 => {
                    let (s, r) = (keys[i].2, keys[i + 1].2);
                    if start(s) == start(r) {
                        return None;
                    }
                    twin[s as usize] = r;
                    twin[r as usize] = s;
                }
                _ => return None,
            }
            i = j;
        }
        if open != fixed {
            return None;
        }
        let mut corner = vec![NONE; fixed];
        for (t, c) in tris.iter().enumerate() {
            for k in 0..3 {
                corner[c[k] as usize] = (3 * t + k) as u32;
            }
        }
        if corner.contains(&NONE) {
            return None;
        }
        Some(Patch {
            sigma: vec![0.0; fixed],
            points,
            tris,
            twin,
            corner,
            fixed,
        })
    }

    fn start(&self, s: u32) -> u32 {
        self.tris[s as usize / 3][s as usize % 3]
    }

    fn end(&self, s: u32) -> u32 {
        self.start(next(s))
    }

    fn at(&self, i: u32) -> P3 {
        self.points[i as usize]
    }

    fn link(&mut self, s: u32, r: u32) {
        self.twin[s as usize] = r;
        if r != NONE {
            self.twin[r as usize] = s;
        }
    }

    /// The points joined to `p` by an edge, sorted.
    fn neighbours(&self, p: u32, out: &mut Vec<u32>) {
        out.clear();
        let first = self.corner[p as usize];
        let limit = self.tris.len() + 1;
        // Round the fan one way: from a side leaving p, across the side entering p in the same
        // triangle, to the side leaving p in the next triangle.
        let mut s = first;
        for _ in 0..limit {
            out.push(self.end(s));
            let back = prev(s);
            let r = self.twin[back as usize];
            if r == NONE {
                // An open fan (p is on the ring): the far end, then round the other way.
                out.push(self.start(back));
                let mut s = first;
                for _ in 0..limit {
                    let r = self.twin[s as usize];
                    if r == NONE {
                        break;
                    }
                    s = next(r);
                    out.push(self.end(s));
                }
                break;
            }
            if r == first {
                break;
            }
            s = r;
        }
        out.sort_unstable();
        out.dedup();
    }

    /// Splits triangle `t` round a new point `x` into three.
    fn split(&mut self, t: usize, x: P3, sigma: f64) {
        let p = self.points.len() as u32;
        self.points.push(x);
        self.sigma.push(sigma);
        self.corner.push(NONE);
        let [a, b, c] = self.tris[t];
        let (t, t1, t2) = (t as u32, self.tris.len() as u32, self.tris.len() as u32 + 1);
        let (ab, bc, ca) = (
            self.twin[3 * t as usize],
            self.twin[3 * t as usize + 1],
            self.twin[3 * t as usize + 2],
        );
        self.tris[t as usize] = [a, b, p];
        self.tris.push([b, c, p]);
        self.tris.push([c, a, p]);
        self.twin.extend([NONE; 6]);
        self.link(3 * t, ab);
        self.link(3 * t1, bc);
        self.link(3 * t2, ca);
        self.link(3 * t + 1, 3 * t1 + 2);
        self.link(3 * t1 + 1, 3 * t2 + 2);
        self.link(3 * t2 + 1, 3 * t + 2);
        self.corner[a as usize] = 3 * t;
        self.corner[b as usize] = 3 * t1;
        self.corner[c as usize] = 3 * t2;
        self.corner[p as usize] = 3 * t + 2;
    }

    /// Replaces the edge of side `s` (a to b, triangles (a, b, c) and (b, a, d)) by c-d:
    /// triangles (a, d, c) and (d, b, c).
    fn flip(&mut self, s: u32) {
        let r = self.twin[s as usize];
        let (t, u) = (s / 3, r / 3);
        let (a, b, c) = (self.start(s), self.end(s), self.start(prev(s)));
        let d = self.start(prev(r));
        let (bc, ca) = (self.twin[next(s) as usize], self.twin[prev(s) as usize]);
        let (ad, db) = (self.twin[next(r) as usize], self.twin[prev(r) as usize]);
        self.tris[t as usize] = [a, d, c];
        self.tris[u as usize] = [d, b, c];
        self.link(3 * t, ad);
        self.link(3 * t + 1, 3 * u + 2);
        self.link(3 * t + 2, ca);
        self.link(3 * u, db);
        self.link(3 * u + 1, bc);
        self.corner[a as usize] = 3 * t;
        self.corner[d as usize] = 3 * t + 1;
        self.corner[c as usize] = 3 * t + 2;
        self.corner[b as usize] = 3 * u + 1;
    }

    /// Whether the edge of side `s` should be flipped: it is not Delaunay (the angles opposite it
    /// add up to more than 180°), or its two triangles fold over each other (more than 90°
    /// apart, as beside a small triangle filling a notch of a rim once the patch round it has
    /// been faired flat) and the flipped pair would not. (Flipping straight back would fold the
    /// pair again, which can_flip refuses unless a crease round it is as sharp already.)
    fn wants_flip(&self, s: u32) -> bool {
        let r = self.twin[s as usize];
        let (a, b) = (self.at(self.start(s)), self.at(self.end(s)));
        let (c, d) = (self.at(self.start(prev(s))), self.at(self.start(prev(r))));
        angle(sub(a, c), sub(b, c)) + angle(sub(b, d), sub(a, d)) > PI + 1e-9
            || (dot(normal(a, b, c), normal(b, a, d)) < 0.0
                && dot(normal(a, d, c), normal(d, b, c)) > 0.0)
    }

    /// Whether the edge of side `s` can be flipped: the new edge is not there yet (in the patch
    /// or, between ring corners, in the mesh), both new triangles have area and (unless the old
    /// pair is folded) face the way the old pair faced, and the sharpest crease among the edges
    /// the flip touches (the edge itself and the four round the pair) gets no sharper, or stays
    /// within 60°.
    fn can_flip(&self, s: u32, forbidden: &Forbidden, around: &mut Vec<u32>) -> bool {
        let r = self.twin[s as usize];
        let (a, b) = (self.start(s), self.end(s));
        let (c, d) = (self.start(prev(s)), self.start(prev(r)));
        if c == d {
            return false;
        }
        let (pa, pb, pc, pd) = (self.at(a), self.at(b), self.at(c), self.at(d));
        let (Some(nt), Some(nu)) = (unit(normal(pa, pb, pc)), unit(normal(pb, pa, pd))) else {
            return false;
        };
        let (Some(n1), Some(n2)) = (unit(normal(pa, pd, pc)), unit(normal(pd, pb, pc))) else {
            return false;
        };
        let facing = add(nt, nu);
        if dot(nt, nu) >= 0.0 && !(dot(n1, facing) > 0.0 && dot(n2, facing) > 0.0) {
            return false;
        }
        if !sliver_free(pa, pd, pc) || !sliver_free(pd, pb, pc) {
            return false;
        }
        // Cosines of the sharpest crease before and after: between the pair, and between each
        // outer edge's triangle in the pair and the triangle beside it.
        let (mut before, mut after) = (dot(nt, nu), dot(n1, n2));
        for (side, old, new) in [
            (next(r), nu, n1),
            (prev(r), nu, n2),
            (next(s), nt, n2),
            (prev(s), nt, n1),
        ] {
            let beside = self.twin[side as usize];
            if beside != NONE {
                let [p, q, o] = self.tris[beside as usize / 3].map(|i| self.at(i));
                let Some(m) = unit(normal(p, q, o)) else {
                    return false;
                };
                before = before.min(dot(old, m));
                after = after.min(dot(new, m));
            }
        }
        if after < before.min(0.5) {
            return false;
        }
        if (c as usize) < self.fixed
            && (d as usize) < self.fixed
            && forbidden.contains(c as usize, d as usize)
        {
            return false;
        }
        self.neighbours(c, around);
        around.binary_search(&d).is_err()
    }

    /// Flips edges that are not Delaunay, or fold, until none that can be flipped is left (or a
    /// generous budget runs out).
    fn relax(&mut self, forbidden: &Forbidden) {
        let mut stack: Vec<u32> = (0..self.twin.len() as u32)
            .filter(|&s| {
                let r = self.twin[s as usize];
                r != NONE && s < r
            })
            .collect();
        let mut budget = 8 * self.tris.len() + 64;
        let mut around = Vec::new();
        while let Some(s) = stack.pop() {
            let r = self.twin[s as usize];
            if r == NONE || !self.wants_flip(s) || !self.can_flip(s, forbidden, &mut around) {
                continue;
            }
            if budget == 0 {
                break;
            }
            budget -= 1;
            self.flip(s);
            let (t, u) = (s / 3, r / 3);
            for side in [3 * t, 3 * t + 2, 3 * u, 3 * u + 1] {
                if self.twin[side as usize] != NONE {
                    stack.push(side);
                }
            }
        }
    }

    /// Per ring corner: whether the mesh beside both of its ring edges meets this (plain) patch
    /// within 60°, so the faired patch should run on smoothly from the surrounding surface there.
    /// The patch's direction at a corner is the area-weighted normal of all its triangles there,
    /// not that of one small triangle, which may only fill a notch of a jagged rim. Where they
    /// meet at a sharper angle (a wall standing on the hole's rim, a sharp lip, or a surface that
    /// folds back under the hole) the rim is a crease, and the patch is left to meet the surface
    /// at an angle rather than be bent to continue it.
    fn smooth_corners(&self, across: &[Option<P3>]) -> Vec<bool> {
        let fixed = self.fixed;
        let mut direction = vec![[0.0; 3]; fixed];
        for t in &self.tris {
            let [a, b, c] = t.map(|v| self.at(v));
            let n = normal(a, b, c);
            for &v in t {
                if (v as usize) < fixed {
                    direction[v as usize] = add(direction[v as usize], n);
                }
            }
        }
        (0..fixed)
            .map(|i| {
                unit(direction[i]).is_some_and(|n| {
                    [across[i], across[(i + fixed - 1) % fixed]]
                        .iter()
                        .all(|a| a.is_some_and(|a| dot(n, a) >= 0.5))
                })
            })
            .collect()
    }

    /// Liepa's refinement: every triangle whose centroid lies further than σ / α from each of
    /// its corners (σ the wanted edge length, interpolated from the ring) splits round the
    /// centroid; then the edges are relaxed and the patch faired a little, and again, until no
    /// triangle splits. This is where the patch gets its shape: each pass settles it on its
    /// coarser points, which a few iterations do, and the next pass's points start on that shape
    /// and only add finer detail. (Fairing the finest patch on to convergence instead lets the
    /// umbrellas' pull along the surface undo the shape: a big patch sags.) Returns whether any
    /// point was added.
    fn refine(
        &mut self,
        outer: &Outer,
        smooth: &[bool],
        forbidden: &Forbidden,
        cancel: &AtomicBool,
    ) -> bool {
        let fixed = self.fixed;
        let ring_length = (0..fixed)
            .map(|i| length(sub(self.points[(i + 1) % fixed], self.points[i])))
            .sum::<f64>()
            / fixed as f64;
        for i in 0..fixed {
            self.sigma[i] = outer.sigma[i].max(1e-3 * ring_length);
        }
        // About one triangle per 0.43 σ² of patch: a hole that would need more than PATCH_MAX
        // gets a proportionally longer σ.
        let mean = self.sigma.iter().sum::<f64>() / fixed as f64;
        let area: f64 = self
            .tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| self.at(i));
                0.5 * length(normal(a, b, c))
            })
            .sum();
        let estimate = area / (0.43 * mean * mean);
        if estimate > PATCH_MAX as f64 {
            let grow = (estimate / PATCH_MAX as f64).sqrt();
            for s in &mut self.sigma {
                *s *= grow;
            }
        }
        for _ in 0..PASSES {
            if cancel.load(Ordering::Relaxed) {
                return false;
            }
            let count = self.tris.len();
            let mut any = false;
            for t in 0..count {
                let [a, b, c] = self.tris[t];
                let (pa, pb, pc) = (self.at(a), self.at(b), self.at(c));
                let x = scale(add(add(pa, pb), pc), 1.0 / 3.0);
                let (sa, sb, sc) = (
                    self.sigma[a as usize],
                    self.sigma[b as usize],
                    self.sigma[c as usize],
                );
                let sx = (sa + sb + sc) / 3.0;
                if [(pa, sa), (pb, sb), (pc, sc)].iter().all(|&(p, s)| {
                    let d = ALPHA * length(sub(x, p));
                    d > sx && d > s
                }) {
                    self.split(t, x, sx);
                    any = true;
                }
            }
            if !any {
                break;
            }
            self.relax(forbidden);
            self.fair(outer, smooth, STEP_ITERATIONS);
            if self.tris.len() >= PATCH_MAX {
                break;
            }
        }
        self.points.len() > fixed
    }

    /// Fairs the new points: minimises the sum of squared umbrella vectors (each point's offset
    /// from the mean of its neighbours) over the new points and the `smooth` ring corners, with
    /// the ring fixed. A smooth corner's umbrella takes in its mesh neighbours too, so there the
    /// patch runs on from the surrounding surface without a crease: a least-squares
    /// bi-Laplacian. With no smooth corner the rows are as many as the unknowns, and the solution
    /// is the membrane through the ring. Solved by conjugate gradients on the normal equations
    /// (CGLS), starting from where the points are.
    ///
    /// Each umbrella is divided by the edge length wanted at its point. An umbrella is about the
    /// surface's curvature times the square of the local edge length, and a point stands for
    /// about that length squared of surface, so the plain sum would weigh the bending of the
    /// patch's coarse middle far above that of its fine rim and let the middle sag; divided, the
    /// sum measures the bending the same everywhere. (Weighting neighbours by their present
    /// distance instead pulls close points ever closer from one pass to the next until they bunch
    /// up; the wanted length does not move.)
    fn fair(&mut self, outer: &Outer, smooth: &[bool], iterations: usize) {
        let fixed = self.fixed;
        let unknowns = self.points.len() - fixed;
        if unknowns == 0 || iterations == 0 {
            return;
        }
        // One row per umbrella: unknowns (column, weight) and the fixed part.
        let mut start = vec![0usize];
        let mut columns: Vec<u32> = Vec::new();
        let mut weights: Vec<f64> = Vec::new();
        let mut constant: Vec<P3> = Vec::new();
        let mut around = Vec::new();
        for v in 0..self.points.len() {
            // Only ring corners have an entry; a crease corner has no umbrella row.
            if smooth.get(v) == Some(&false) {
                continue;
            }
            self.neighbours(v as u32, &mut around);
            if v < fixed && !around.iter().any(|&w| w as usize >= fixed) {
                continue;
            }
            // A new point's umbrella is the mean of its neighbours less the point. A ring corner's
            // is the mean of its offsets to its resampled mesh neighbours (among them its two ring
            // neighbours) and to its other patch neighbours, kept as offsets so the new points'
            // share of the corner's own position goes into the fixed part.
            let p = self.points[v];
            let corner = v < fixed;
            let k = 1.0 / self.sigma[v];
            let (mut c, mut count) = if corner {
                (outer.offsets[v], outer.count[v])
            } else {
                ([0.0; 3], 0.0)
            };
            let (before, after) = ((v + fixed - 1) % fixed, (v + 1) % fixed);
            let mut new = 0.0;
            for &w in &around {
                let w = w as usize;
                if corner && (w == before || w == after) {
                    continue;
                }
                count += 1.0;
                if w >= fixed {
                    columns.push((w - fixed) as u32);
                    new += 1.0;
                } else if corner {
                    c = add(c, sub(self.points[w], p));
                } else {
                    c = add(c, self.points[w]);
                }
            }
            weights.resize(columns.len(), k / count);
            if corner {
                c = scale(sub(c, scale(p, new)), k / count);
            } else {
                c = scale(c, k / count);
                columns.push((v - fixed) as u32);
                weights.push(-k);
            }
            constant.push(c);
            start.push(columns.len());
        }
        let rows = constant.len();
        let apply = |x: &[P3], out: &mut Vec<P3>| {
            out.clear();
            for r in 0..rows {
                let mut s = [0.0; 3];
                for e in start[r]..start[r + 1] {
                    s = add(s, scale(x[columns[e] as usize], weights[e]));
                }
                out.push(s);
            }
        };
        let apply_transposed = |y: &[P3], out: &mut [P3]| {
            out.fill([0.0; 3]);
            for r in 0..rows {
                for e in start[r]..start[r + 1] {
                    let j = columns[e] as usize;
                    out[j] = add(out[j], scale(y[r], weights[e]));
                }
            }
        };
        // Solve A x ≈ −c, each coordinate on its own.
        let mut x: Vec<P3> = self.points[fixed..].to_vec();
        let mut q = Vec::with_capacity(rows);
        apply(&x, &mut q);
        let mut residual: Vec<P3> = (0..rows)
            .map(|i| sub(scale(constant[i], -1.0), q[i]))
            .collect();
        let mut s = vec![[0.0; 3]; unknowns];
        apply_transposed(&residual, &mut s);
        let mut direction = s.clone();
        let mut gamma = squares(&s);
        let limit = gamma.map(|g| g * FAIR_TOLERANCE * FAIR_TOLERANCE);
        for _ in 0..iterations {
            if (0..3).all(|k| gamma[k] <= limit[k]) {
                break;
            }
            apply(&direction, &mut q);
            let delta = squares(&q);
            let step: P3 = std::array::from_fn(|k| {
                if delta[k] > 0.0 {
                    gamma[k] / delta[k]
                } else {
                    0.0
                }
            });
            for (xi, di) in x.iter_mut().zip(&direction) {
                *xi = add(*xi, times(step, *di));
            }
            for (ri, qi) in residual.iter_mut().zip(&q) {
                *ri = sub(*ri, times(step, *qi));
            }
            apply_transposed(&residual, &mut s);
            let next_gamma = squares(&s);
            let keep: P3 = std::array::from_fn(|k| {
                if gamma[k] > 0.0 {
                    next_gamma[k] / gamma[k]
                } else {
                    0.0
                }
            });
            for (di, si) in direction.iter_mut().zip(&s) {
                *di = add(*si, times(keep, *di));
            }
            gamma = next_gamma;
        }
        self.points[fixed..].copy_from_slice(&x);
    }

    /// When the faired patch folds somewhere, alternates untangling the folds, edge relaxation
    /// and fairing a few times, and keeps the state that folds least (the patch as it was, if
    /// none folds less). Fairing moves the points along the surface as well as onto it, which can
    /// leave thin triangles folded over edges that are no longer Delaunay; flipping those edges
    /// lets the next fairing even the points out again.
    fn settle(
        &mut self,
        outer: &Outer,
        smooth: &[bool],
        forbidden: &Forbidden,
        across: &[Option<P3>],
    ) {
        let inner = |p: &Patch| p.folds(across).map_or(usize::MAX, |f| f.inner);
        let mut least = inner(self);
        let mut trial = self.clone();
        for _ in 0..SETTLE_ROUNDS {
            for step in 0..2 {
                if least == 0 {
                    return;
                }
                if step == 0 {
                    trial.untangle();
                } else {
                    trial.relax(forbidden);
                    trial.fair(outer, smooth, SETTLE_ITERATIONS);
                }
                let folds = inner(&trial);
                if folds < least {
                    least = folds;
                    *self = trial.clone();
                }
            }
        }
    }

    /// Moves the new points of folded triangles to the mean of their neighbours, a few times. A
    /// fold left at a small notch of the ring, between a triangle of ring corners and one that
    /// the fairing laid flat beside it, often opens once its own few points give way.
    fn untangle(&mut self) {
        let mut around = Vec::new();
        for _ in 0..UNTANGLE_ROUNDS {
            let unit_normal = |t: usize| {
                let [a, b, c] = self.tris[t].map(|i| self.at(i));
                unit(normal(a, b, c))
            };
            let mut folded: Vec<u32> = Vec::new();
            for (s, &r) in self.twin.iter().enumerate() {
                if r == NONE || (s as u32) > r {
                    continue;
                }
                let (t, u) = (s / 3, r as usize / 3);
                if let (Some(n), Some(m)) = (unit_normal(t), unit_normal(u))
                    && dot(n, m) < 0.0
                {
                    folded.extend(self.tris[t].iter().chain(&self.tris[u]));
                }
            }
            folded.retain(|&v| v as usize >= self.fixed);
            folded.sort_unstable();
            folded.dedup();
            if folded.is_empty() {
                return;
            }
            let moved: Vec<P3> = folded
                .iter()
                .map(|&v| {
                    self.neighbours(v, &mut around);
                    let sum = around.iter().fold([0.0; 3], |s, &w| add(s, self.at(w)));
                    scale(sum, 1.0 / around.len().max(1) as f64)
                })
                .collect();
            for (&v, p) in folded.iter().zip(moved) {
                self.points[v as usize] = p;
            }
        }
    }

    /// Where the patch folds over. A patch that meets the mesh at a sharp convex edge, as across
    /// the lip of a thin shell, does not fold there: that edge is the surface's own. None when a
    /// triangle has no area to speak of.
    fn folds(&self, across: &[Option<P3>]) -> Option<Folds> {
        let mut normals = Vec::with_capacity(self.tris.len());
        for t in &self.tris {
            let [a, b, c] = t.map(|i| self.at(i));
            if !sliver_free(a, b, c) {
                return None;
            }
            normals.push(unit(normal(a, b, c))?);
        }
        let mut folds = Folds { inner: 0, back: 0 };
        for (s, &r) in self.twin.iter().enumerate() {
            let t = s / 3;
            if r == NONE {
                // The ring corner lies in the mesh triangle's plane; the patch triangle's third
                // corner in front of that plane makes the edge concave.
                let i = self.tris[t][s % 3];
                let apex = self.at(self.start(prev(s as u32)));
                if let Some(a) = across[i as usize]
                    && dot(normals[t], a) < -0.5
                    && dot(sub(apex, self.at(i)), a) > 0.0
                {
                    folds.back += 1;
                }
            } else if (s as u32) < r && dot(normals[t], normals[r as usize / 3]) < 0.0 {
                folds.inner += 1;
            }
        }
        Some(folds)
    }

    /// Whether the faired patch may replace the plain one, as far as its own shape goes: finite,
    /// no triangle without area, folding inside itself at no more edges than `inner` (the plain
    /// triangulation's count: a ring that pinches folds every patch), and no point further from
    /// the loop's plane than the loop is wide. Folding back over the mesh at a ring edge is
    /// allowed: every patch does where the surface beside the hole curls over it.
    fn acceptable(&self, shape: &Shape, across: &[Option<P3>], inner: usize) -> bool {
        self.points.iter().flatten().all(|x| x.is_finite())
            && self.folds(across).is_some_and(|f| f.inner <= inner)
            && self.points[self.fixed..]
                .iter()
                .all(|&p| shape.distance(p) <= shape.size)
    }
}

/// Where a patch folds over.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Folds {
    /// Edges between neighbouring patch triangles more than 90° apart.
    inner: usize,
    /// Ring edges where the patch triangle folds back over the mesh triangle across: more than
    /// 120° from it, and in front of it.
    back: usize,
}

/// The angle between two vectors, 0..π.
fn angle(a: P3, b: P3) -> f64 {
    length(cross(a, b)).atan2(dot(a, b))
}

/// Whether a triangle's sine at `a` is not lost in rounding.
fn sliver_free(a: P3, b: P3, c: P3) -> bool {
    let (ab, ac) = (sub(b, a), sub(c, a));
    let n = cross(ab, ac);
    dot(n, n) > 1e-24 * dot(ab, ab) * dot(ac, ac)
}

fn unit(a: P3) -> Option<P3> {
    let l = length(a);
    (l > 0.0 && l.is_finite()).then(|| scale(a, 1.0 / l))
}

fn dist2(a: P3, b: P3) -> f64 {
    let d = sub(a, b);
    dot(d, d)
}

fn times(a: P3, b: P3) -> P3 {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}

/// Per coordinate, the sum of squares.
fn squares(v: &[P3]) -> P3 {
    v.iter().fold([0.0; 3], |s, x| add(s, times(*x, *x)))
}

// ---------------------------------------------------------------------------------------------
// Joining the patches to the mesh

/// The mesh with every closed hole's piece applied in loop order: new points appended, zipped
/// triangles replaced in place, new triangles appended. A zip that would replace a triangle an
/// earlier one already replaced counts as failed.
fn assemble(mesh: &Mesh, holes: Vec<Hole>) -> Filled {
    let triangles = mesh.triangle_count();
    let extra_points: usize = holes
        .iter()
        .map(|h| match h {
            Hole::Closed(p) => p.points.len(),
            _ => 0,
        })
        .sum();
    let extra_triangles: usize = holes
        .iter()
        .map(|h| match h {
            Hole::Closed(p) => p.triangles.len(),
            _ => 0,
        })
        .sum();
    let mut positions = Vec::with_capacity(mesh.positions.len() + 3 * extra_points);
    positions.extend_from_slice(&mesh.positions);
    let mut indices = Vec::with_capacity(mesh.indices.len() + 3 * extra_triangles);
    indices.extend_from_slice(&mesh.indices);
    let mut origin: Vec<u32> = Vec::with_capacity(triangles + extra_triangles);
    origin.extend(0..triangles as u32);
    let (mut closed, mut failed) = (0, 0);
    let mut replaced: Vec<u32> = Vec::new();
    for hole in holes {
        let piece = match hole {
            Hole::Skipped => continue,
            Hole::Failed => {
                failed += 1;
                continue;
            }
            Hole::Closed(piece) => piece,
        };
        if piece.replaced.iter().any(|(t, _)| replaced.contains(t)) {
            failed += 1;
            continue;
        }
        let base = (positions.len() / 3) as u32;
        let own = piece.vertices.len() as u32;
        let global = |i: u32| {
            if i < own {
                piece.vertices[i as usize]
            } else {
                base + i - own
            }
        };
        for p in &piece.points {
            positions.extend_from_slice(p);
        }
        for &(t, c) in &piece.replaced {
            indices[t as usize * 3..t as usize * 3 + 3].copy_from_slice(&c.map(global));
            replaced.push(t);
        }
        for (c, &o) in piece.triangles.iter().zip(&piece.origin) {
            indices.extend_from_slice(&c.map(global));
            origin.push(o);
        }
        closed += 1;
    }
    Filled {
        added: origin.len() - triangles,
        mesh: Mesh { positions, indices },
        origin,
        closed,
        failed,
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    use super::*;
    use crate::mesh::{make_box, stats};
    use crate::ops::concat;
    use crate::repair::exact::testing::{Rng, torus};
    use crate::repair::topology::{is_consistent, orientation_patches};
    use crate::repair::{RepairError, orient, remap_selection, silent};

    fn cube() -> Mesh {
        make_box([0.0; 3], [20.0; 3])
    }

    /// The mesh without some of make_box's faces: 0 −z, 1 +z, 2 −y, 3 +y, 4 +x, 5 −x, two
    /// triangles each (vertices stay).
    fn without_faces(mut mesh: Mesh, faces: &[usize]) -> Mesh {
        mesh.indices = mesh
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .enumerate()
            .filter(|(t, _)| !faces.contains(&(t / 2)))
            .flat_map(|(_, c)| *c)
            .collect();
        mesh
    }

    /// The mesh without the triangles `drop` picks (vertices stay).
    fn without(mesh: &Mesh, drop: impl Fn(usize, [P3; 3]) -> bool) -> Mesh {
        let mut out = mesh.clone();
        out.indices = mesh
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .enumerate()
            .filter(|&(t, _)| !drop(t, corners(mesh, t as u32)))
            .flat_map(|(_, c)| *c)
            .collect();
        out
    }

    fn flip(mesh: &mut Mesh, t: usize) {
        mesh.indices.swap(t * 3 + 1, t * 3 + 2);
    }

    fn all(mesh: &Mesh) -> Outcome {
        silent(|c| close_all(mesh, &[], c)).unwrap()
    }

    fn trivial(mesh: &Mesh) -> Outcome {
        silent(|c| close_trivial(mesh, &[], c)).unwrap()
    }

    fn curved(mesh: &Mesh) -> Outcome {
        silent(|c| close_curved(mesh, &[], c)).unwrap()
    }

    fn holes(mesh: &Mesh) -> usize {
        boundary_loops(mesh, &Edges::new(mesh)).len()
    }

    /// Every edge has exactly two sides running opposite ways, and no triangle repeats a corner.
    fn assert_closed(mesh: &Mesh) {
        let edges = Edges::new(mesh);
        for e in 0..edges.len() as u32 {
            assert_eq!(edges.degree(e), 2, "edge {:?}", edges.ends(e));
            assert!(is_consistent(mesh, &edges, e), "edge {:?}", edges.ends(e));
        }
        for t in mesh.indices.as_chunks::<3>().0 {
            assert!(t[0] != t[1] && t[1] != t[2] && t[2] != t[0], "{t:?}");
        }
    }

    /// No triangle from `from` on has exactly zero area.
    fn assert_areas(mesh: &Mesh, from: usize) {
        for t in from..mesh.triangle_count() {
            let [a, b, c] = corners(mesh, t as u32);
            assert!(!collinear(a, b, c), "triangle {t} has no area");
        }
    }

    /// No two new triangles (from `from` on) that share an edge are more than 90° apart.
    fn assert_no_folds(mesh: &Mesh, from: usize) {
        let edges = Edges::new(mesh);
        let n = |t: u32| {
            let [a, b, c] = corners(mesh, t);
            unit(normal(a, b, c)).unwrap()
        };
        for e in 0..edges.len() as u32 {
            if let [s, r] = *edges.sides(e)
                && (s / 3) as usize >= from
                && (r / 3) as usize >= from
            {
                assert!(dot(n(s / 3), n(r / 3)) > 0.0, "fold at {:?}", edges.ends(e));
            }
        }
    }

    fn close_to(a: f64, b: f64, relative: f64) -> bool {
        (a - b).abs() <= relative * b.abs().max(1e-300)
    }

    /// A closed UV sphere of radius r round the origin facing outward: `stacks` bands from the
    /// north pole (vertex 0) to the south pole (the last vertex), `slices` round.
    fn sphere(r: f64, stacks: u32, slices: u32) -> Mesh {
        let mut positions = vec![0.0, 0.0, r as f32];
        for i in 1..stacks {
            let phi = PI * i as f64 / stacks as f64;
            for j in 0..slices {
                let theta = TAU * j as f64 / slices as f64;
                positions.extend_from_slice(&[
                    (r * phi.sin() * theta.cos()) as f32,
                    (r * phi.sin() * theta.sin()) as f32,
                    (r * phi.cos()) as f32,
                ]);
            }
        }
        positions.extend_from_slice(&[0.0, 0.0, -r as f32]);
        let south = 1 + (stacks - 1) * slices;
        let at = |i: u32, j: u32| 1 + (i - 1) * slices + j % slices;
        let mut indices = Vec::new();
        for j in 0..slices {
            indices.extend_from_slice(&[0, at(1, j), at(1, j + 1)]);
            indices.extend_from_slice(&[south, at(stacks - 1, j + 1), at(stacks - 1, j)]);
        }
        for i in 1..stacks - 1 {
            for j in 0..slices {
                let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
        Mesh { positions, indices }
    }

    /// A closed cylinder round the z axis facing outward: radius r, height h, `segments` round,
    /// `rows` bands up the side, each cap a fan round its centre (bottom centre, then top centre,
    /// after the side's vertices). The side cell (row i, segment j) is triangles 2 (i ·
    /// segments + j) and the one after.
    fn cylinder(r: f64, h: f64, segments: u32, rows: u32) -> Mesh {
        let mut positions = Vec::new();
        for i in 0..=rows {
            for j in 0..segments {
                let theta = TAU * j as f64 / segments as f64;
                positions.extend_from_slice(&[
                    (r * theta.cos()) as f32,
                    (r * theta.sin()) as f32,
                    (h * i as f64 / rows as f64) as f32,
                ]);
            }
        }
        let bottom = (rows + 1) * segments;
        positions.extend_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.0, h as f32]);
        let at = |i: u32, j: u32| i * segments + j % segments;
        let mut indices = Vec::new();
        for i in 0..rows {
            for j in 0..segments {
                let (a, b, c, d) = (at(i, j), at(i, j + 1), at(i + 1, j + 1), at(i + 1, j));
                indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
        for j in 0..segments {
            indices.extend_from_slice(&[bottom, at(0, j + 1), at(0, j)]);
            indices.extend_from_slice(&[bottom + 1, at(rows, j), at(rows, j + 1)]);
        }
        Mesh { positions, indices }
    }

    /// The cylinder without its top cap (the triangles round the top centre).
    fn open_prism(segments: u32) -> (Mesh, Mesh) {
        let closed = cylinder(10.0, 5.0, segments, 1);
        let top = closed.vertex_count() as u32 - 1;
        let mut open = closed.clone();
        open.indices = closed
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|t| !t.contains(&top))
            .flat_map(|c| *c)
            .collect();
        (closed, open)
    }

    /// A closed slab facing outward: a grid of `w` × `h` square cells of side `cell` on top
    /// (z = height) and bottom (z = 0) joined round the rim. Top cell (i, j) is triangles
    /// 2 (j · w + i) and the one after.
    fn slab(w: u32, h: u32, cell: f32, height: f32) -> Mesh {
        let mut positions = Vec::new();
        for z in [height, 0.0] {
            for j in 0..=h {
                for i in 0..=w {
                    positions.extend_from_slice(&[i as f32 * cell, j as f32 * cell, z]);
                }
            }
        }
        let top = |i: u32, j: u32| j * (w + 1) + i;
        let bottom = |i: u32, j: u32| (w + 1) * (h + 1) + j * (w + 1) + i;
        let mut indices = Vec::new();
        for j in 0..h {
            for i in 0..w {
                let (a, b, c, d) = (top(i, j), top(i + 1, j), top(i + 1, j + 1), top(i, j + 1));
                indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
        for j in 0..h {
            for i in 0..w {
                let (a, b, c, d) = (
                    bottom(i, j),
                    bottom(i + 1, j),
                    bottom(i + 1, j + 1),
                    bottom(i, j + 1),
                );
                indices.extend_from_slice(&[a, c, b, a, d, c]);
            }
        }
        // The rim counter-clockwise seen from above.
        let mut rim: Vec<(u32, u32)> = (0..w).map(|i| (i, 0)).collect();
        rim.extend((0..h).map(|j| (w, j)));
        rim.extend((1..=w).rev().map(|i| (i, h)));
        rim.extend((1..=h).rev().map(|j| (0, j)));
        for k in 0..rim.len() {
            let (p, q) = (rim[k], rim[(k + 1) % rim.len()]);
            let (bp, bq, tq, tp) = (
                bottom(p.0, p.1),
                bottom(q.0, q.1),
                top(q.0, q.1),
                top(p.0, p.1),
            );
            indices.extend_from_slice(&[bp, bq, tq, bp, tq, tp]);
        }
        Mesh { positions, indices }
    }

    /// A regular octahedron with outward triangles; the first four meet at the top vertex 4.
    fn octahedron() -> Mesh {
        Mesh {
            positions: vec![
                1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0, 0.0,
                0.0, -1.0,
            ],
            indices: vec![
                0, 2, 4, 2, 1, 4, 1, 3, 4, 3, 0, 4, // top
                2, 0, 5, 1, 2, 5, 3, 1, 5, 0, 3, 5, // bottom
            ],
        }
    }

    fn radius(p: P3) -> f64 {
        length(p)
    }

    fn axis_distance(p: P3) -> f64 {
        p[0].hypot(p[1])
    }

    fn new_points(out: &Outcome, before: &Mesh) -> Vec<P3> {
        (before.vertex_count()..out.mesh.vertex_count())
            .map(|v| vertex(&out.mesh, v as u32))
            .collect()
    }

    // -----------------------------------------------------------------------------------------
    // The simple cases

    #[test]
    fn a_cube_missing_a_face_gets_two_outward_triangles() {
        // The top face adds a third of the volume, so a patch facing in would leave 2,666.7.
        let open = without_faces(cube(), &[1]);
        for out in [all(&open), trivial(&open)] {
            assert!(out.changed);
            assert_eq!(out.message, "Closed 1 hole, added 2 triangles.");
            assert!(out.warnings.is_empty());
            assert_eq!(out.mesh.positions, open.positions, "no new vertices");
            assert_eq!(out.mesh.indices[..30], open.indices[..]);
            let expected: Vec<u32> = (0..10).chain([NEW, NEW]).collect();
            assert_eq!(out.origin, Origin::Map(expected));
            assert_closed(&out.mesh);
            assert_eq!(stats(&out.mesh).volume, 8000.0);
        }
        // The fixture's open bottom.
        let out = all(&without_faces(cube(), &[0]));
        assert_closed(&out.mesh);
        assert_eq!(stats(&out.mesh).volume, 8000.0);
    }

    #[test]
    fn fill_reports_counts_and_the_origin_map() {
        let open = without_faces(cube(), &[1, 4]);
        let filled = silent(|c| fill(&open, Which::All, c)).unwrap();
        assert_eq!((filled.closed, filled.added, filled.failed), (1, 4, 0));
        let expected: Vec<u32> = (0..8).chain([NEW; 4]).collect();
        assert_eq!(filled.origin, expected);
        assert_closed(&filled.mesh);
        assert!(close_to(stats(&filled.mesh).volume, 8000.0, 1e-12));
        // Nothing to do: the same mesh and the identity map.
        let none = silent(|c| fill(&cube(), Which::All, c)).unwrap();
        assert_eq!(none.mesh, cube());
        assert_eq!(none.origin, (0..12).collect::<Vec<u32>>());
        assert_eq!((none.closed, none.added, none.failed), (0, 0, 0));
    }

    #[test]
    fn holes_of_three_five_and_many_edges_get_flat_patches() {
        // Three edges: one triangle.
        let mut m = octahedron();
        m.indices.drain(0..3);
        let out = all(&m);
        assert_eq!(out.message, "Closed 1 hole, added 1 triangle.");
        assert_closed(&out.mesh);
        assert!(close_to(stats(&out.mesh).volume, 4.0 / 3.0, 1e-6));
        assert_eq!(trivial(&m).mesh, out.mesh);
        // Five and 64 and 1,000 edges: n − 2 triangles in the cap's plane, facing up, no new
        // vertices, and the prism's volume back.
        for segments in [5u32, 64, 1000] {
            let (closed, open) = open_prism(segments);
            let out = all(&open);
            assert_eq!(
                out.message,
                format!(
                    "Closed 1 hole, added {} triangles.",
                    fmt_count(segments as u64 - 2)
                )
            );
            assert_eq!(out.mesh.vertex_count(), open.vertex_count());
            assert_closed(&out.mesh);
            assert_areas(&out.mesh, open.triangle_count());
            for t in open.triangle_count()..out.mesh.triangle_count() {
                let [a, b, c] = corners(&out.mesh, t as u32);
                assert!(a[2] == 5.0 && b[2] == 5.0 && c[2] == 5.0);
                assert!(normal(a, b, c)[2] > 0.0);
            }
            assert!(close_to(
                stats(&out.mesh).volume,
                stats(&closed).volume,
                1e-9
            ));
        }
    }

    #[test]
    fn close_trivial_fills_only_three_edges_and_flat_four_edges() {
        let mut three = octahedron();
        three.indices.drain(0..3);
        let flat_four = without_faces(make_box([5.0; 3], [7.0; 3]), &[1]);
        // A cube whose top corner (20, 20, 20) is raised to z = 40: its open top is a 4-edge
        // loop bent by 55° or more whichever way it is split.
        let mut bent = make_box([30.0, 0.0, 0.0], [50.0, 20.0, 20.0]);
        bent.positions[6 * 3 + 2] = 40.0;
        let bent = without_faces(bent, &[1]);
        let (_, five) = open_prism(5);
        let m = concat(&[&three, &flat_four, &bent, &five]).unwrap();
        assert_eq!(holes(&m), 4);
        let out = trivial(&m);
        assert_eq!(out.message, "Closed 2 holes, added 3 triangles.");
        assert_eq!(holes(&out.mesh), 2);
        // The rest is for Close all holes.
        let rest = all(&out.mesh);
        assert!(
            rest.message.starts_with("Closed 2 holes"),
            "{}",
            rest.message
        );
        assert_closed(&rest.mesh);
        assert_areas(&rest.mesh, m.triangle_count());
        // Nothing trivial left.
        let again = trivial(&out.mesh);
        assert!(!again.changed);
        assert_eq!(again.message, "No trivial holes to close.");
        assert_eq!(again.origin, Origin::Same);
        assert_eq!(again.mesh, out.mesh);
    }

    #[test]
    fn a_closed_mesh_has_no_holes_to_close() {
        for out in [
            all(&cube()),
            trivial(&cube()),
            silent(|c| close_defined(&cube(), &[3], c)).unwrap(),
        ] {
            assert!(!out.changed);
            assert_eq!(out.message, "No holes to close.");
            assert_eq!(out.mesh, cube());
            assert_eq!(out.origin, Origin::Same);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Flat holes

    #[test]
    fn an_l_shaped_opening_gets_a_flat_patch_without_new_vertices() {
        // A 20 × 20 × 5 mm slab; its top loses an L of cells: 4 × 2 along the bottom, 2 × 2 more
        // up the left side.
        let s = slab(8, 8, 2.5, 5.0);
        let cell = |i: u32, j: u32| -> bool {
            ((2..6).contains(&i) && (2..4).contains(&j))
                || ((2..4).contains(&i) && (4..6).contains(&j))
        };
        let open = without(&s, |t, _| {
            t < 128 && cell((t as u32 / 2) % 8, (t as u32 / 2) / 8)
        });
        assert_eq!(s.triangle_count() - open.triangle_count(), 24);
        assert_eq!(holes(&open), 1);
        let out = all(&open);
        assert!(out.changed);
        assert_eq!(out.mesh.positions, s.positions, "no new vertices");
        assert_closed(&out.mesh);
        assert_areas(&out.mesh, open.triangle_count());
        let mut area = 0.0;
        for t in open.triangle_count()..out.mesh.triangle_count() {
            let [a, b, c] = corners(&out.mesh, t as u32);
            assert!(a[2] == 5.0 && b[2] == 5.0 && c[2] == 5.0, "off the top");
            let n = normal(a, b, c);
            assert!(n[2] > 0.0, "facing down");
            area += 0.5 * n[2];
        }
        // The L is 12 cells of 6.25 mm², covered once.
        assert!(close_to(area, 75.0, 1e-12), "{area}");
        assert!(close_to(stats(&out.mesh).volume, 2000.0, 1e-12));
    }

    #[test]
    fn the_plane_polygon_check_is_exact() {
        let square = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert!(is_simple(&square));
        // A bow tie, a corner touching another edge, a repeated corner, a corner folding back.
        assert!(!is_simple(&[
            [0.0, 0.0],
            [1.0, 1.0],
            [1.0, 0.0],
            [0.0, 1.0]
        ]));
        assert!(!is_simple(&[
            [0.0, 0.0],
            [4.0, 0.0],
            [4.0, 4.0],
            [2.0, 0.0],
            [0.0, 4.0]
        ]));
        assert!(!is_simple(&[
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 2.0],
            [0.0, 0.0],
            [-2.0, 2.0]
        ]));
        assert!(!is_simple(&[
            [0.0, 0.0],
            [2.0, 0.0],
            [1.0, 0.0],
            [1.0, 1.0]
        ]));
        assert!(!is_simple(&[
            [0.0, 0.0],
            [1.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0]
        ]));
        // A straight run along an edge is fine.
        assert!(is_simple(&[[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [1.0, 1.0]]));
        // One unit in the last place decides.
        let tiny = 1f64.next_up() - 1.0;
        assert!(!is_simple(&[
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 2.0],
            [1.0, 0.0],
            [0.0, 2.0]
        ]));
        assert!(is_simple(&[
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 2.0],
            [1.0, tiny],
            [0.0, 2.0]
        ]));
    }

    #[test]
    fn the_plane_triangulation_covers_a_non_convex_polygon_once() {
        // An L and a comb, counter-clockwise.
        let l = vec![
            [0.0, 0.0],
            [3.0, 0.0],
            [3.0, 1.0],
            [1.0, 1.0],
            [1.0, 3.0],
            [0.0, 3.0],
        ];
        let mut comb = vec![[0.0, 0.0], [9.0, 0.0], [9.0, 1.0]];
        for k in (0..4).rev() {
            let x = 2.0 * k as f64 + 1.0;
            comb.extend_from_slice(&[[x + 1.0, 1.0], [x + 0.5, 5.0], [x, 1.0]]);
        }
        comb.push([0.0, 1.0]);
        for polygon in [l, comb] {
            let n = polygon.len();
            let triangles = cdt(&polygon).unwrap();
            assert_eq!(triangles.len(), n - 2);
            let area2 = |t: [u32; 3]| {
                let [a, b, c] = t.map(|i| polygon[i as usize]);
                (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
            };
            assert!(triangles.iter().all(|&t| area2(t) > 0.0));
            let total: f64 = triangles.iter().map(|&t| area2(t)).sum();
            assert!(close_to(total, signed_area(&polygon), 1e-12));
            assert!(
                Patch::new(
                    polygon.iter().map(|p| [p[0], p[1], 0.0]).collect(),
                    triangles
                )
                .is_some()
            );
        }
    }

    // -----------------------------------------------------------------------------------------
    // Curved holes

    #[test]
    fn a_hole_in_a_cylinder_wall_follows_the_cylinder() {
        let closed = cylinder(10.0, 20.0, 48, 24);
        // 10 segments (75° of arc) by 8 rows.
        let open = without(&closed, |t, _| {
            let (i, j) = ((t / 2) / 48, (t / 2) % 48);
            t < 48 * 24 * 2 && (8..16).contains(&i) && j < 10
        });
        assert_eq!(holes(&open), 1);
        let out = all(&open);
        assert!(out.changed && out.warnings.is_empty(), "{out:?}");
        assert_closed(&out.mesh);
        assert_areas(&out.mesh, open.triangle_count());
        assert_no_folds(&out.mesh, open.triangle_count());
        let points = new_points(&out, &open);
        assert!(points.len() > 20, "refined: {} new points", points.len());
        // A flat or minimal patch would sag to 7.9 mm from the axis in the middle.
        let worst = points
            .iter()
            .map(|&p| (axis_distance(p) - 10.0).abs())
            .fold(0.0, f64::max);
        println!(
            "cylinder wall: {} new points, {worst:.4} mm off the wall at worst",
            points.len()
        );
        assert!(worst < 0.15, "{worst}");
        assert!(close_to(
            stats(&out.mesh).volume,
            stats(&closed).volume,
            0.005
        ));
    }

    #[test]
    fn a_bent_hole_in_a_sphere_bulges_with_the_sphere() {
        let closed = sphere(10.0, 40, 80);
        // Every triangle within 40° of longitude and 3 mm of the equator: a loop bent far out of
        // any plane, whose flat patch would lie 7.7 mm from the centre in the middle.
        let open = without(&closed, |_, [a, b, c]| {
            let m = scale(add(add(a, b), c), 1.0 / 3.0);
            m[1].atan2(m[0]).abs() < 40f64.to_radians() && m[2].abs() < 3.0
        });
        assert_eq!(holes(&open), 1);
        let ring = &boundary_loops(&open, &Edges::new(&open))[0];
        assert!(!Shape::new(&Ring::new(&open, ring).points).flat);
        let out = all(&open);
        assert!(out.changed && out.warnings.is_empty(), "{out:?}");
        assert_closed(&out.mesh);
        assert_areas(&out.mesh, open.triangle_count());
        assert_no_folds(&out.mesh, open.triangle_count());
        let points = new_points(&out, &open);
        assert!(points.len() > 20, "refined: {} new points", points.len());
        let worst = points
            .iter()
            .map(|&p| (radius(p) - 10.0).abs())
            .fold(0.0, f64::max);
        println!(
            "sphere: {} new points, {worst:.4} mm off the sphere at worst",
            points.len()
        );
        assert!(worst < 0.15, "{worst}");
        assert!(close_to(
            stats(&out.mesh).volume,
            stats(&closed).volume,
            0.005
        ));
    }

    #[test]
    fn a_nearly_circular_hole_in_a_sphere_counts_as_flat() {
        // Every triangle within 25° of a point on the equator of a fine sphere: the ragged loop
        // strays only about 1% of its diameter from its plane, so it gets the flat patch the
        // design asks for planar-ish loops.
        let closed = sphere(10.0, 80, 160);
        let open = without(&closed, |_, [a, b, c]| {
            let m = scale(add(add(a, b), c), 1.0 / 3.0);
            dot(unit(m).unwrap(), [1.0, 0.0, 0.0]) > 25f64.to_radians().cos()
        });
        let ring = &boundary_loops(&open, &Edges::new(&open))[0];
        let points = Ring::new(&open, ring).points;
        let shape = Shape::new(&points);
        let deviation = points
            .iter()
            .map(|&p| shape.distance(p))
            .fold(0.0, f64::max);
        assert!(deviation < 0.015 * shape.size, "{}", deviation / shape.size);
        assert!(shape.flat && shape.polygon.is_some());
        let out = all(&open);
        assert_eq!(out.mesh.vertex_count(), open.vertex_count());
        assert_closed(&out.mesh);
        assert_areas(&out.mesh, open.triangle_count());
    }

    #[test]
    fn an_l_shaped_hole_in_a_cylinder_wall_follows_the_cylinder() {
        let closed = cylinder(10.0, 20.0, 48, 24);
        let open = without(&closed, |t, _| {
            let (i, j) = ((t / 2) / 48, (t / 2) % 48);
            t < 48 * 24 * 2
                && ((8..16).contains(&i) && j < 4 || (8..11).contains(&i) && (4..12).contains(&j))
        });
        assert_eq!(holes(&open), 1);
        let out = all(&open);
        assert_closed(&out.mesh);
        assert_areas(&out.mesh, open.triangle_count());
        assert_no_folds(&out.mesh, open.triangle_count());
        let points = new_points(&out, &open);
        assert!(points.len() > 20, "refined: {} new points", points.len());
        let worst = points
            .iter()
            .map(|&p| (axis_distance(p) - 10.0).abs())
            .fold(0.0, f64::max);
        println!(
            "L in the wall: {} new points, {worst:.4} mm off the wall at worst",
            points.len()
        );
        assert!(worst < 0.15, "{worst}");
        assert!(close_to(
            stats(&out.mesh).volume,
            stats(&closed).volume,
            0.005
        ));
    }

    #[test]
    fn a_hole_round_most_of_a_cylinder_is_bridged_along_the_wall() {
        // 300° of the wall by 8 rows: seen along its own normal the loop folds over itself, so
        // only the minimum-weight triangulation (or the chord split, for the long one) can fill
        // it. 64 segments gives a loop of 122 edges; 600 segments one of 1,016.
        for segments in [64u32, 600] {
            let closed = cylinder(10.0, 20.0, segments, 24);
            let span = segments * 5 / 6;
            let open = without(&closed, |t, _| {
                let (i, j) = ((t as u32 / 2) / segments, (t as u32 / 2) % segments);
                (t as u32) < segments * 24 * 2 && (8..16).contains(&i) && j < span
            });
            let ring = &boundary_loops(&open, &Edges::new(&open))[0];
            assert_eq!(ring.sides.len() as u32, 2 * (span + 8));
            assert!(Shape::new(&Ring::new(&open, ring).points).polygon.is_none());
            let started = Instant::now();
            let out = all(&open);
            println!(
                "{segments} segments: {:.3} s, {}",
                started.elapsed().as_secs_f64(),
                out.message
            );
            assert!(out.changed && out.warnings.is_empty(), "{out:?}");
            assert_closed(&out.mesh);
            assert_areas(&out.mesh, open.triangle_count());
            assert_no_folds(&out.mesh, open.triangle_count());
            // Every patch triangle runs along the wall instead of cutting across the cylinder.
            for t in open.triangle_count()..out.mesh.triangle_count() {
                let [a, b, c] = corners(&out.mesh, t as u32);
                let r = axis_distance(scale(add(add(a, b), c), 1.0 / 3.0));
                assert!(
                    (9.5..=10.5).contains(&r),
                    "triangle {t} is {r} from the axis"
                );
            }
            assert!(close_to(
                stats(&out.mesh).volume,
                stats(&closed).volume,
                0.01
            ));
        }
    }

    #[test]
    fn a_long_curved_loop_is_cut_into_pieces_then_refined() {
        // 120° of a fine cylinder's wall by 20 rows: a loop of 440 edges, too long for one
        // minimum-weight triangulation (its plane polygon is simple, but triangulating that
        // would start the patch off flat).
        let segments = 600u32;
        let closed = cylinder(10.0, 20.0, segments, 40);
        let open = without(&closed, |t, _| {
            let (i, j) = ((t as u32 / 2) / segments, (t as u32 / 2) % segments);
            (t as u32) < segments * 40 * 2 && (10..30).contains(&i) && j < 200
        });
        let ring = &boundary_loops(&open, &Edges::new(&open))[0];
        assert_eq!(ring.sides.len(), 440);
        let shape = Shape::new(&Ring::new(&open, ring).points);
        assert!(!shape.flat && shape.polygon.is_some());
        let started = Instant::now();
        let out = all(&open);
        let took = started.elapsed().as_secs_f64();
        assert!(out.changed && out.warnings.is_empty(), "{out:?}");
        assert_closed(&out.mesh);
        assert_areas(&out.mesh, open.triangle_count());
        assert_no_folds(&out.mesh, open.triangle_count());
        let points = new_points(&out, &open);
        let worst = points
            .iter()
            .map(|&p| (axis_distance(p) - 10.0).abs())
            .fold(0.0, f64::max);
        println!(
            "long loop: {took:.3} s, {} new points, {worst:.4} mm off the wall at worst",
            points.len()
        );
        assert!(points.len() > 100, "refined: {} new points", points.len());
        // A flat patch would sag 5 mm in the middle.
        assert!(worst < 0.1, "{worst}");
        assert!(close_to(
            stats(&out.mesh).volume,
            stats(&closed).volume,
            0.005
        ));
    }

    /// The minimum-weight triangulation of a bent 400-corner ring, timed: `cargo test --release
    /// --lib repair::holes -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn the_largest_minimum_weight_triangulation_takes_a_fraction_of_a_second() {
        let n = MWT_MAX;
        // A saddle-shaped ring: round in the plane, up and down twice in z.
        let ring: Vec<P3> = (0..n)
            .map(|i| {
                let a = TAU * i as f64 / n as f64;
                [10.0 * a.cos(), 10.0 * a.sin(), 3.0 * (2.0 * a).cos()]
            })
            .collect();
        let never = AtomicBool::new(false);
        let started = Instant::now();
        let triangles = mwt(&ring, &vec![None; n], |_, _| false, &never).unwrap();
        let took = started.elapsed().as_secs_f64();
        println!("minimum-weight triangulation of {n} corners: {took:.3} s");
        assert_eq!(triangles.len(), n - 2);
        assert!(Patch::new(ring, triangles).is_some());
    }

    #[test]
    fn planar_loops_on_curved_surfaces_get_flat_patches() {
        // A hemisphere's rim and a sphere with a cap cut off: both loops lie in a plane, as a
        // planar cut leaves them, and get the flat face such a cut would have.
        let closed = sphere(10.0, 24, 48);
        let hemisphere = without(&closed, |_, [a, b, c]| a[2] + b[2] + c[2] < 0.0);
        // Ring 4 lies at z = 8.66; the band below it has centroids under 8.5, the one above
        // over 8.8.
        let capped = without(&closed, |_, [a, b, c]| a[2] + b[2] + c[2] > 3.0 * 8.6);
        for (open, height) in [(hemisphere, 0.0f64), (capped, (PI / 6.0).cos() * 10.0)] {
            assert_eq!(holes(&open), 1);
            let out = all(&open);
            assert!(out.changed);
            assert_eq!(
                out.mesh.vertex_count(),
                open.vertex_count(),
                "no new vertices"
            );
            assert_closed(&out.mesh);
            assert_areas(&out.mesh, open.triangle_count());
            for t in open.triangle_count()..out.mesh.triangle_count() {
                for p in corners(&out.mesh, t as u32) {
                    assert!((p[2] - height).abs() < 1e-5, "{p:?}");
                }
            }
        }
        // The hemisphere closed flat holds exactly half the sphere.
        let hemisphere = without(&closed, |_, [a, b, c]| a[2] + b[2] + c[2] < 0.0);
        let out = all(&hemisphere);
        assert!(close_to(
            stats(&out.mesh).volume,
            stats(&closed).volume / 2.0,
            1e-6
        ));
    }

    // -----------------------------------------------------------------------------------------
    // Along the curvature

    #[test]
    fn along_the_curvature_a_cap_cut_off_a_sphere_closes_as_a_dome() {
        // The capped sphere of the test above: Close all holes gives it a flat lid at z = 8.66,
        // 1.34 mm below where the sphere was. Along the curvature the patch carries the sphere on.
        let closed = sphere(10.0, 48, 96);
        let capped = without(&closed, |_, [a, b, c]| a[2] + b[2] + c[2] > 3.0 * 8.6);
        let lid = (PI / 6.0).cos() * 10.0;
        let out = curved(&capped);
        assert!(out.changed && out.warnings.is_empty(), "{out:?}");
        assert!(
            out.message.starts_with("Closed 1 hole, added "),
            "{}",
            out.message
        );
        assert_closed(&out.mesh);
        assert_areas(&out.mesh, capped.triangle_count());
        assert_no_folds(&out.mesh, capped.triangle_count());
        let points = new_points(&out, &capped);
        let worst = points
            .iter()
            .map(|&p| (radius(p) - 10.0).abs())
            .fold(0.0, f64::max);
        let top = points.iter().map(|p| p[2]).fold(f64::MIN, f64::max);
        println!(
            "dome: {} new points, top at {top:.3} (lid {lid:.3}), {worst:.4} mm off the sphere at worst",
            points.len()
        );
        // The pole was at 10; the dome may overshoot it a little (0.09 mm here, 0.3 mm on a sphere
        // of half the resolution, whose faceted ring bends more sharply).
        assert!(points.len() > 100, "refined: {} new points", points.len());
        assert!(top > lid + 1.0 && top < 10.2, "{top}");
        assert!(worst < 0.2, "{worst}");
        // Within half a per cent of the whole sphere; the flat lid leaves out 1.3% of it.
        let volume = stats(&out.mesh).volume;
        assert!(close_to(volume, stats(&closed).volume, 0.005), "{volume}");
        assert!(stats(&all(&capped).mesh).volume < 0.99 * stats(&closed).volume);
    }

    #[test]
    fn along_the_curvature_a_hole_in_a_flat_face_stays_in_the_face() {
        // Four top cells out of the middle of a slab: the patch is refined, but every point stays
        // in the top face, as the surface round it is flat.
        let closed = slab(8, 8, 2.0, 3.0);
        let open = without(&closed, |t, _| {
            t < 128 && {
                let (i, j) = ((t / 2) % 8, (t / 2) / 8);
                (3..5).contains(&i) && (3..5).contains(&j)
            }
        });
        assert_eq!(holes(&open), 1);
        let out = curved(&open);
        assert!(out.changed && out.warnings.is_empty(), "{out:?}");
        assert_closed(&out.mesh);
        assert_areas(&out.mesh, open.triangle_count());
        for t in open.triangle_count()..out.mesh.triangle_count() {
            for p in corners(&out.mesh, t as u32) {
                assert!((p[2] - 3.0).abs() < 1e-5, "{p:?}");
            }
        }
        assert!(close_to(
            stats(&out.mesh).volume,
            stats(&closed).volume,
            1e-6
        ));
    }

    #[test]
    fn along_the_curvature_small_holes_close_as_close_all_holes_closes_them() {
        // A missing cube face, a three-edge hole and a curved hole the size of the cells round
        // it: no point is added where the patch's triangles are as small as the mesh's already.
        let face = without_faces(cube(), &[1]);
        let three = without(&octahedron(), |t, _| t == 0);
        for open in [face, three] {
            let out = curved(&open);
            assert_eq!(out.mesh.vertex_count(), open.vertex_count());
            assert_closed(&out.mesh);
            assert_eq!(out.mesh.triangle_count(), all(&open).mesh.triangle_count());
        }
    }

    #[test]
    fn along_the_curvature_the_selection_limits_which_holes_close() {
        // Two caps cut off a sphere, north and south; a selected triangle beside the north one.
        let closed = sphere(10.0, 24, 48);
        let open = without(&closed, |_, [a, b, c]| {
            (a[2] + b[2] + c[2]).abs() > 3.0 * 8.6
        });
        assert_eq!(holes(&open), 2);
        let north = (0..open.triangle_count() as u32)
            .find(|&t| corners(&open, t).iter().filter(|p| p[2] > 8.6).count() == 2)
            .unwrap();
        let out = silent(|c| close_curved(&open, &[north], c)).unwrap();
        assert!(out.changed);
        assert_eq!(holes(&out.mesh), 1);
        let points = new_points(&out, &open);
        assert!(!points.is_empty() && points.iter().all(|p| p[2] > 8.0));
        // And nothing selected next to a hole: nothing to close.
        let away = silent(|c| close_curved(&open, &[open.triangle_count() as u32 / 2], c)).unwrap();
        assert!(!away.changed);
        assert_eq!(away.message, "No holes border the selected triangles.");
    }

    /// The open cap of a sphere of radius 10 above the wavy cut z = 6 + 1.5 sin 3θ, facing
    /// outward: `segments` round, `rows` from the rim (vertices 0..segments, exactly on the cut)
    /// to the pole (the last vertex).
    fn wavy_cap(segments: u32, rows: u32) -> Mesh {
        let mut positions = Vec::new();
        for i in 0..rows {
            for j in 0..segments {
                let theta = TAU * j as f64 / segments as f64;
                let rim = ((6.0 + 1.5 * (3.0 * theta).sin()) / 10.0).acos();
                let phi = rim * (1.0 - i as f64 / rows as f64);
                positions.extend(
                    [phi.sin() * theta.cos(), phi.sin() * theta.sin(), phi.cos()]
                        .map(|x| (10.0 * x) as f32),
                );
            }
        }
        positions.extend_from_slice(&[0.0, 0.0, 10.0]);
        let pole = rows * segments;
        let at = |i: u32, j: u32| i * segments + j % segments;
        let mut indices = Vec::new();
        for i in 0..rows - 1 {
            for j in 0..segments {
                let (a, b, c, d) = (at(i, j), at(i, j + 1), at(i + 1, j + 1), at(i + 1, j));
                indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
        for j in 0..segments {
            indices.extend_from_slice(&[at(rows - 1, j), at(rows - 1, j + 1), pole]);
        }
        Mesh { positions, indices }
    }

    #[test]
    fn a_curved_hole_on_a_sharp_lip_is_still_faired() {
        // A small cap of a sphere cut off along a wavy line: the sphere meets the hole at 117° to
        // 139° all round, a sharp convex lip, and the cut bends about 9% of its width out of any
        // plane. The patch is refined and faired like any curved hole: meeting the sphere at a
        // knife edge is the cap's own shape, not a fold.
        let open = wavy_cap(120, 12);
        let loops = boundary_loops(&open, &Edges::new(&open));
        assert_eq!(loops.len(), 1);
        let ring = Ring::new(&open, &loops[0]);
        assert!(!Shape::new(&ring.points).flat);
        let (low, high) = ring
            .points
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), p| {
                (lo.min(p[2]), hi.max(p[2]))
            });
        let out = all(&open);
        assert!(out.changed && out.warnings.is_empty(), "{out:?}");
        assert_closed(&out.mesh);
        assert_areas(&out.mesh, open.triangle_count());
        assert_no_folds(&out.mesh, open.triangle_count());
        let points = new_points(&out, &open);
        assert!(points.len() > 20, "refined: {} new points", points.len());
        // The patch spans the rim instead of bulging past it.
        for p in &points {
            assert!((low..=high).contains(&p[2]), "{p:?} outside {low}..{high}");
        }
        assert!(stats(&out.mesh).volume > 0.0);
        // The same cut through a sphere's grid leaves a jagged rim whose teeth stick out of any
        // smooth patch, so every faired patch would pleat round them: the hole still closes,
        // over the ring's own corners, without a fold.
        let jagged = without(&sphere(10.0, 60, 120), |_, [a, b, c]| {
            let m = scale(add(add(a, b), c), 1.0 / 3.0);
            m[2] < 6.0 + 1.5 * (3.0 * m[1].atan2(m[0])).sin()
        });
        assert_eq!(holes(&jagged), 1);
        let out = all(&jagged);
        assert!(out.changed && out.warnings.is_empty(), "{out:?}");
        assert_closed(&out.mesh);
        assert_areas(&out.mesh, jagged.triangle_count());
        assert_no_folds(&out.mesh, jagged.triangle_count());
        assert!(stats(&out.mesh).volume > 0.0);
    }

    #[test]
    fn a_long_hole_in_a_finely_divided_wall_closes_without_folds() {
        // A cylinder of 2,000 segments by 10 rows: cells 0.03 mm round by 2 mm up, the long thin
        // triangles CAD exports are full of. A band of 90° by 4 rows, a loop of 1,008 edges, is
        // cut out. Refined or (where refinement would crowd its points against the finely divided
        // rim) kept over the ring's own corners, the patch closes the wall without folding and
        // follows it.
        let closed = cylinder(10.0, 20.0, 2000, 10);
        let open = without(&closed, |t, _| {
            let (i, j) = ((t as u32 / 2) / 2000, (t as u32 / 2) % 2000);
            (t as u32) < 2000 * 10 * 2 && (3..7).contains(&i) && j < 500
        });
        let ring = &boundary_loops(&open, &Edges::new(&open))[0];
        assert_eq!(ring.sides.len(), 1008);
        let out = all(&open);
        assert!(out.changed && out.warnings.is_empty(), "{out:?}");
        assert_closed(&out.mesh);
        assert_areas(&out.mesh, open.triangle_count());
        assert_no_folds(&out.mesh, open.triangle_count());
        for t in open.triangle_count()..out.mesh.triangle_count() {
            let [a, b, c] = corners(&out.mesh, t as u32);
            let r = axis_distance(scale(add(add(a, b), c), 1.0 / 3.0));
            assert!(
                (9.9..=10.1).contains(&r),
                "triangle {t} is {r} from the axis"
            );
        }
        assert!(close_to(
            stats(&out.mesh).volume,
            stats(&closed).volume,
            0.001
        ));
    }

    // -----------------------------------------------------------------------------------------
    // Topology and orientation

    #[test]
    fn two_holes_sharing_a_vertex_close_into_one_manifold_shell() {
        let mut m = octahedron();
        m.indices.drain(6..9);
        m.indices.drain(0..3);
        assert_eq!(holes(&m), 2);
        for out in [all(&m), trivial(&m)] {
            assert_eq!(out.message, "Closed 2 holes, added 2 triangles.");
            assert_closed(&out.mesh);
            assert!(close_to(stats(&out.mesh).volume, 4.0 / 3.0, 1e-6));
        }
        // Two 3 × 3 blocks of cells on a sphere's equator meeting only at the vertex where ring
        // 15 crosses segment 0, each curved hole refined and faired on its own. Band cell (i, j)
        // is triangles 120 + 2 ((i − 1) 60 + j) and the one after.
        let closed = sphere(10.0, 30, 60);
        let open = without(&closed, |t, _| {
            if t < 120 {
                return false;
            }
            let c = (t - 120) / 2;
            let (i, j) = (c / 60 + 1, c % 60);
            ((12..15).contains(&i) && (57..60).contains(&j))
                || ((15..18).contains(&i) && (0..3).contains(&j))
        });
        let loops = boundary_loops(&open, &Edges::new(&open));
        assert_eq!(loops.len(), 2);
        assert!(loops.iter().all(|l| l.sides.len() == 12));
        let out = all(&open);
        assert!(out.message.starts_with("Closed 2 holes"), "{}", out.message);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert_closed(&out.mesh);
        assert_areas(&out.mesh, open.triangle_count());
        assert_no_folds(&out.mesh, open.triangle_count());
        for p in new_points(&out, &open) {
            assert!((9.8..=10.2).contains(&radius(p)), "{p:?}");
        }
    }

    #[test]
    fn a_hole_beside_turned_triangles_follows_the_majority() {
        // The +x triangle on the open top's edge 5-6 is turned, so that border side runs against
        // the other three. The patch faces the way the other three ask (up), and once the turned
        // triangle is turned back (what Fix flipped triangles does) the cube is whole.
        let mut open = without_faces(cube(), &[1]);
        flip(&mut open, 7);
        let out = all(&open);
        assert_eq!(out.message, "Closed 1 hole, added 2 triangles.");
        for t in 10..12 {
            let [a, b, c] = corners(&out.mesh, t);
            assert!(normal(a, b, c)[2] > 0.0);
        }
        let mut fixed = out.mesh.clone();
        let edges = Edges::new(&fixed);
        let patches = orientation_patches(&fixed, &edges);
        let inverted =
            silent(|c| orient::inverted(&fixed, &edges, &patches, orient::Nested::Outward, c))
                .unwrap();
        assert_eq!(
            (0..12).filter(|&t| inverted[t]).collect::<Vec<_>>(),
            vec![7]
        );
        flip(&mut fixed, 7);
        assert_closed(&fixed);
        assert_eq!(stats(&fixed).volume, 8000.0);
    }

    #[test]
    fn an_inside_out_mesh_gets_an_inside_out_patch() {
        let mut open = without_faces(cube(), &[1]);
        for t in 0..10 {
            flip(&mut open, t);
        }
        let out = all(&open);
        assert_closed(&out.mesh);
        assert_eq!(stats(&out.mesh).volume, -8000.0);
        // The same for a curved hole on an inside-out sphere.
        let mut closed = sphere(10.0, 30, 60);
        for t in 0..closed.triangle_count() {
            flip(&mut closed, t);
        }
        let open = without(&closed, |_, [a, b, c]| {
            let m = scale(add(add(a, b), c), 1.0 / 3.0);
            dot(unit(m).unwrap(), [0.0, 1.0, 0.0]) > 0.9
        });
        let out = all(&open);
        assert_closed(&out.mesh);
        assert!(close_to(
            stats(&out.mesh).volume,
            stats(&closed).volume,
            0.005
        ));
    }

    #[test]
    fn a_patch_never_reuses_an_existing_edge() {
        // Two triangles over the diagonal 0-2: closing their outline over that diagonal again
        // would give it four sides. The patch takes the other diagonal.
        let m = Mesh {
            positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0],
            indices: vec![0, 1, 2, 0, 2, 3],
        };
        for out in [all(&m), trivial(&m)] {
            assert_eq!(out.message, "Closed 1 hole, added 2 triangles.");
            assert_closed(&out.mesh);
            for t in out.mesh.indices[6..].as_chunks::<3>().0 {
                assert!(t.contains(&1) && t.contains(&3), "{t:?}");
            }
        }
        // A triangle sticking into a hole with two border edges: its third edge is a mesh
        // edge between two of the hole's corners that are not neighbours on the loop.
        let mut s = slab(4, 4, 1.0, 1.0);
        let open = without(&s, |t, _| t < 32 && [5, 6, 9, 10].contains(&(t / 2)));
        s = open.clone();
        let spike = s.vertex_count() as u32;
        s.positions.extend_from_slice(&[2.0, 2.0, 1.0]);
        // Corner (1, 1) is vertex 6, (2, 1) is vertex 7: the spike rests on the hole's lower edge.
        s.indices.extend_from_slice(&[6, 7, spike]);
        let out = all(&s);
        assert!(out.changed && out.warnings.is_empty(), "{out:?}");
        assert_closed(&out.mesh);
        assert_areas(&out.mesh, s.triangle_count());
    }

    #[test]
    fn closing_twice_changes_nothing_the_second_time() {
        let closed = sphere(10.0, 30, 60);
        let ragged = without(&closed, |_, [a, b, c]| {
            let m = scale(add(add(a, b), c), 1.0 / 3.0);
            dot(unit(m).unwrap(), [0.0, 0.6, 0.8]) > 0.85
        });
        let mut many = octahedron();
        many.indices.drain(6..9);
        many.indices.drain(0..3);
        for mesh in [
            without_faces(cube(), &[1, 3]),
            ragged,
            many,
            open_prism(40).1,
        ] {
            for action in [close_all, close_trivial] {
                let first = silent(|c| action(&mesh, &[], c)).unwrap();
                let second = silent(|c| action(&first.mesh, &[], c)).unwrap();
                assert!(!second.changed, "{}", second.message);
                assert_eq!(second.mesh, first.mesh);
                assert_eq!(second.origin, Origin::Same);
            }
            let first = all(&mesh);
            assert_eq!(all(&first.mesh).message, "No holes to close.");
        }
    }

    // -----------------------------------------------------------------------------------------
    // Slits

    #[test]
    fn a_t_junction_slit_is_zipped_without_zero_area_triangles() {
        // The +x face uses a vertex 8 halfway along the top edge 5-6, the top face does not: a
        // slit 5 → 6 → 8 whose three corners lie on one line.
        let mut m = cube();
        m.positions.extend_from_slice(&[20.0, 10.0, 20.0]);
        let t = m
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .position(|c| *c == [1, 6, 5])
            .unwrap();
        m.indices[t * 3..t * 3 + 3].copy_from_slice(&[1, 6, 8]);
        m.indices.extend_from_slice(&[1, 8, 5]);
        assert_eq!(holes(&m), 1);
        for out in [all(&m), trivial(&m)] {
            assert_eq!(out.message, "Closed 1 hole, added 1 triangle.");
            assert_eq!(out.mesh.vertex_count(), m.vertex_count());
            assert_closed(&out.mesh);
            assert_areas(&out.mesh, 0);
            assert_eq!(stats(&out.mesh).volume, 8000.0);
            // The top triangle (4, 5, 6) became (5, 8, 4) in place, and (8, 6, 4) follows it.
            let Origin::Map(origin) = &out.origin else {
                panic!("no origin map")
            };
            assert_eq!(origin.len(), 14);
            assert_eq!(origin[13], 2);
            assert_eq!(&out.mesh.indices[6..9], &[5, 8, 4]);
            assert_eq!(&out.mesh.indices[39..42], &[8, 6, 4]);
            assert_eq!(remap_selection(&[2], &out.origin, 14), vec![2, 13]);
        }
    }

    #[test]
    fn a_longer_slit_zips_both_sides() {
        // The +x face meets the top edge at y = 5 and y = 15, the top face at y = 10.
        let mut m = cube();
        m.positions
            .extend_from_slice(&[20.0, 5.0, 20.0, 20.0, 15.0, 20.0, 20.0, 10.0, 20.0]);
        let (a, b, c) = (8u32, 9, 10);
        let side = m
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .position(|t| *t == [1, 6, 5])
            .unwrap();
        m.indices[side * 3..side * 3 + 3].copy_from_slice(&[1, 6, b]);
        m.indices.extend_from_slice(&[1, b, a, 1, a, 5]);
        let top = m
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .position(|t| *t == [4, 5, 6])
            .unwrap();
        m.indices[top * 3..top * 3 + 3].copy_from_slice(&[4, 5, c]);
        m.indices.extend_from_slice(&[4, c, 6]);
        let loops = boundary_loops(&m, &Edges::new(&m));
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].sides.len(), 5);
        let out = all(&m);
        assert_eq!(out.message, "Closed 1 hole, added 3 triangles.");
        assert_closed(&out.mesh);
        assert_areas(&out.mesh, 0);
        assert_eq!(stats(&out.mesh).volume, 8000.0);
        assert_eq!(all(&out.mesh).message, "No holes to close.");
    }

    #[test]
    fn a_slit_with_two_corners_in_one_place_stays_open() {
        // Two distinct vertices at the same point on the slit: no pairing of the pieces closes it.
        let mut m = cube();
        m.positions
            .extend_from_slice(&[20.0, 10.0, 20.0, 20.0, 10.0, 20.0]);
        let side = m
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .position(|t| *t == [1, 6, 5])
            .unwrap();
        m.indices[side * 3..side * 3 + 3].copy_from_slice(&[1, 6, 8]);
        m.indices.extend_from_slice(&[1, 8, 5]);
        let top = m
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .position(|t| *t == [4, 5, 6])
            .unwrap();
        m.indices[top * 3..top * 3 + 3].copy_from_slice(&[4, 5, 9]);
        m.indices.extend_from_slice(&[4, 9, 6]);
        assert_eq!(holes(&m), 1);
        let out = all(&m);
        assert!(!out.changed);
        assert_eq!(out.message, "No holes were closed.");
        assert_eq!(
            out.warnings,
            vec!["1 hole could not be closed.".to_string()]
        );
        assert_eq!(out.mesh, m);
    }

    // -----------------------------------------------------------------------------------------
    // Selection, cancel, progress

    #[test]
    fn the_selection_limits_which_holes_close() {
        let a = without_faces(cube(), &[1]);
        let b = without_faces(make_box([30.0; 3], [50.0; 3]), &[1]);
        let m = concat(&[&a, &b]).unwrap();
        // Triangle 13 is the second cube's −y triangle (0, 5, 4) along its open top's edge.
        let out = silent(|c| close_all(&m, &[13], c)).unwrap();
        assert_eq!(out.message, "Closed 1 hole, added 2 triangles.");
        assert_eq!(holes(&out.mesh), 1);
        assert_eq!(remap_selection(&[13], &out.origin, 22), vec![13]);
        let new: Vec<u32> = out.mesh.indices[60..].to_vec();
        assert!(
            new.iter().all(|&v| v >= 8),
            "the second cube's hole: {new:?}"
        );
        let defined = silent(|c| close_defined(&m, &[13], c)).unwrap();
        assert_eq!(defined.mesh, out.mesh);
        let small = silent(|c| close_trivial(&m, &[13], c)).unwrap();
        assert_eq!(small.message, "Closed 1 hole, added 2 triangles.");
        assert_eq!(small.mesh.indices[..60], m.indices[..]);
        assert!(small.mesh.indices[60..].iter().all(|&v| v >= 8));
        // A selection away from every hole, or none, for Close defined holes.
        let away = silent(|c| close_all(&m, &[0], c)).unwrap();
        assert!(!away.changed);
        assert_eq!(away.message, "No holes border the selected triangles.");
        let away = silent(|c| close_trivial(&m, &[0, 1], c)).unwrap();
        assert_eq!(
            away.message,
            "No trivial holes border the selected triangles."
        );
        let none = silent(|c| close_defined(&m, &[], c)).unwrap();
        assert!(!none.changed);
        assert_eq!(none.message, "No triangles are selected.");
        // Ids past the end touch nothing.
        let past = silent(|c| close_defined(&m, &[999], c)).unwrap();
        assert!(!past.changed);
        // Close defined holes fills a hole of any size. The prism's side triangle 1 has an edge
        // on the open top.
        let (_, five) = open_prism(5);
        let out = silent(|c| close_defined(&five, &[1], c)).unwrap();
        assert_eq!(out.message, "Closed 1 hole, added 3 triangles.");
    }

    #[test]
    fn a_cancelled_fill_stops() {
        let open = without_faces(cube(), &[1]);
        let cancel = AtomicBool::new(true);
        let mut control = Control::new(&cancel, |_| {});
        assert_eq!(
            close_all(&open, &[], &mut control),
            Err(RepairError::Cancelled)
        );
        assert_eq!(
            close_trivial(&open, &[], &mut control),
            Err(RepairError::Cancelled)
        );
        assert_eq!(
            close_defined(&open, &[0], &mut control),
            Err(RepairError::Cancelled)
        );
        assert_eq!(
            fill(&open, Which::All, &mut control),
            Err(RepairError::Cancelled)
        );
    }

    #[test]
    fn progress_rises_to_one() {
        let open = without_faces(cube(), &[1]);
        let never = AtomicBool::new(false);
        let mut seen = Vec::new();
        {
            let mut control = Control::new(&never, |f| seen.push(f));
            close_all(&open, &[], &mut control).unwrap();
        }
        assert!(seen.is_sorted() && seen.last() == Some(&1.0), "{seen:?}");
    }

    // -----------------------------------------------------------------------------------------
    // The triangulations on their own

    #[test]
    fn the_minimum_weight_triangulation_avoids_folds_and_forbidden_chords() {
        // A planar L: every valid triangulation is flat, so the one found covers the L once.
        let l: Vec<P3> = [
            [0.0, 0.0],
            [3.0, 0.0],
            [3.0, 1.0],
            [1.0, 1.0],
            [1.0, 3.0],
            [0.0, 3.0],
        ]
        .iter()
        .map(|p| [p[0], p[1], 0.0])
        .collect();
        let never = AtomicBool::new(false);
        let up = Some([0.0, 0.0, 1.0]);
        let triangles = mwt(&l, &[up; 6], |_, _| false, &never).unwrap();
        assert_eq!(triangles.len(), 4);
        let mut area = 0.0;
        for t in &triangles {
            let [a, b, c] = t.map(|i| l[i as usize]);
            let n = normal(a, b, c);
            assert!(n[2] > 0.0, "{t:?} folds over");
            area += 0.5 * n[2];
        }
        assert!(close_to(area, 5.0, 1e-12));
        // Forbidding the chord 0-2 still leaves two flat triangulations. Every one of the L's
        // uses the chord 0-3 from its reflex corner, so without it the best left folds over.
        let triangles = mwt(&l, &[up; 6], |i, k| (i.min(k), i.max(k)) == (0, 2), &never).unwrap();
        assert_eq!(triangles.len(), 4);
        assert!(
            triangles
                .iter()
                .all(|t| !(t.contains(&0) && t.contains(&2)))
        );
        let folded = mwt(&l, &[up; 6], |i, k| (i.min(k), i.max(k)) == (0, 3), &never).unwrap();
        assert!(folded.iter().all(|t| !(t.contains(&0) && t.contains(&3))));
        assert!(folded.iter().any(|t| {
            let [a, b, c] = t.map(|i| l[i as usize]);
            normal(a, b, c)[2] < 0.0
        }));
        // Forbidding every chord leaves nothing for six corners, and triangles with no area
        // are never used: four corners on one line plus one off it has exactly one way.
        assert!(mwt(&l, &[up; 6], |_, _| true, &never).is_none());
        let fan: Vec<P3> = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [3.0, 0.0, 0.0],
            [1.5, 1.0, 0.0],
        ];
        let triangles = mwt(&fan, &[None; 5], |_, _| false, &never).unwrap();
        for t in &triangles {
            assert!(t.contains(&4), "{t:?} has no area");
        }
        assert!(mwt(&fan[..4], &[None; 4], |_, _| false, &never).is_none());
    }

    #[test]
    fn forbidden_chords_are_flipped_out_of_the_plane_triangulation() {
        // Random convex polygons with random chords of their Delaunay triangulation forbidden:
        // whenever a triangulation comes back it is valid, turns counter-clockwise, and has no
        // forbidden chord left, however the flips renumbered its triangles.
        let mut rng = Rng(5);
        let mut cleared = 0;
        for round in 0..2000 {
            let n = 6 + rng.below(15) as usize;
            let polygon: Vec<P2> = (0..n)
                .map(|i| {
                    let a = TAU * (i as f64 + 0.8 * rng.unit()) / n as f64;
                    let r = 10.0 + rng.unit();
                    [r * a.cos(), r * a.sin()]
                })
                .collect();
            let triangles = cdt(&polygon).unwrap();
            let mut pairs: Vec<(u32, u32)> = triangles
                .iter()
                .flat_map(|t| (0..3).map(move |k| (t[k], t[(k + 1) % 3])))
                .filter(|&(a, b)| !adjacent(a as usize, b as usize, n) && rng.chance(0.4))
                .map(|(a, b)| (a.min(b), a.max(b)))
                .collect();
            pairs.sort_unstable();
            pairs.dedup();
            let forbidden = Forbidden { pairs };
            let Some(out) = unforbidden(&polygon, triangles, &forbidden) else {
                continue;
            };
            cleared += 1;
            assert_eq!(out.len(), n - 2, "round {round}");
            for t in &out {
                let [a, b, c] = t.map(|i| polygon[i as usize]);
                assert_eq!(orient2d(a, b, c), 1, "round {round}: {t:?}");
                for k in 0..3 {
                    let (u, v) = (t[k] as usize, t[(k + 1) % 3] as usize);
                    assert!(
                        adjacent(u, v, n) || !forbidden.contains(u, v),
                        "round {round}: chord {u}-{v} left"
                    );
                }
            }
            let flat: Vec<P3> = polygon.iter().map(|p| [p[0], p[1], 0.0]).collect();
            assert!(Patch::new(flat, out).is_some(), "round {round}");
        }
        assert!(cleared > 1000, "{cleared}");
    }

    #[test]
    fn crossing_patch_triangles_are_caught_exactly() {
        let piece = |points: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>| Piece {
            vertices: Vec::new(),
            points,
            origin: vec![NEW; triangles.len()],
            triangles,
            replaced: Vec::new(),
        };
        let none = Mesh::default();
        // A triangle in z = 0 pierced by an upright one.
        let pierced = piece(
            vec![
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [0.0, 2.0, 0.0],
                [0.5, 0.5, -1.0],
                [0.5, 0.5, 1.0],
                [0.5, -1.0, 0.0],
            ],
            vec![[0, 1, 2], [3, 4, 5]],
        );
        assert!(crosses_itself(&none, &pierced));
        // A fan round a raised point over a square: neighbours share edges, nothing crosses.
        let mut square = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.5, 0.5, 0.2],
        ];
        let fan = vec![[0, 1, 4], [1, 2, 4], [2, 3, 4], [3, 0, 4]];
        assert!(!crosses_itself(&none, &piece(square.clone(), fan.clone())));
        // The fan with its point pushed flat past the square's right side folds onto itself.
        square[4] = [2.0, 0.5, 0.0];
        assert!(crosses_itself(&none, &piece(square, fan)));
    }

    #[test]
    fn a_long_ring_is_cut_across_its_waist() {
        // A 300 × 10 rectangle walked round in steps of about 0.8 and 0.2: the best chord joins
        // the middles of the long sides. 800 corners are searched in full; 3,000, every third.
        for m in [800usize, 3000] {
            let points: Vec<P3> = (0..m)
                .map(|i| {
                    let s = 620.0 * i as f64 / m as f64;
                    if s < 300.0 {
                        [s, 0.0, 0.0]
                    } else if s < 310.0 {
                        [300.0, s - 300.0, 0.0]
                    } else if s < 610.0 {
                        [610.0 - s, 10.0, 0.0]
                    } else {
                        [0.0, 620.0 - s, 0.0]
                    }
                })
                .collect();
            let part: Vec<u32> = (0..m as u32).collect();
            let (a, b) = chord(&points, &part, &Forbidden { pairs: Vec::new() }).unwrap();
            let (p, q) = (points[a], points[b]);
            assert!(
                (p[0] - 150.0).abs() < 1.0 && (q[0] - 150.0).abs() < 1.0,
                "{m}: {p:?} {q:?}"
            );
            assert_eq!((p[1] - q[1]).abs(), 10.0);
        }
    }

    #[test]
    fn a_patch_splits_and_flips_consistently() {
        // A square split round its centre, then each new edge flipped back and forth: the patch
        // must stay a valid triangulation of its ring throughout.
        let points: Vec<P3> = vec![
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 2.0, 0.0],
            [0.0, 2.0, 0.0],
        ];
        let mut patch = Patch::new(points.clone(), vec![[0, 1, 2], [0, 2, 3]]).unwrap();
        patch.sigma = vec![0.1; 4];
        patch.split(0, [1.4, 0.6, 0.0], 0.1);
        patch.split(1, [0.6, 1.4, 0.0], 0.1);
        let check = |p: &Patch| {
            for (s, &r) in p.twin.iter().enumerate() {
                if r != NONE {
                    assert_eq!(p.twin[r as usize], s as u32);
                    assert_eq!(p.start(r), p.end(s as u32));
                    assert_eq!(p.end(r), p.start(s as u32));
                }
            }
            for (v, &s) in p.corner.iter().enumerate() {
                assert_eq!(p.start(s), v as u32);
            }
            let ring_sides = p.twin.iter().filter(|&&r| r == NONE).count();
            assert_eq!(ring_sides, 4);
        };
        check(&patch);
        // Point 4 sits in the lower triangle (0, 1, 2), point 5 in the upper one (0, 2, 3).
        let mut nb = Vec::new();
        patch.neighbours(4, &mut nb);
        assert_eq!(nb, vec![0, 1, 2]);
        patch.neighbours(0, &mut nb);
        assert_eq!(nb, vec![1, 2, 3, 4, 5]);
        patch.neighbours(1, &mut nb);
        assert_eq!(nb, vec![0, 2, 4]);
        let forbidden = Forbidden { pairs: Vec::new() };
        let interior: Vec<u32> = (0..patch.twin.len() as u32)
            .filter(|&s| patch.twin[s as usize] != NONE)
            .collect();
        for s in interior {
            if patch.twin[s as usize] != NONE && patch.can_flip(s, &forbidden, &mut nb) {
                patch.flip(s);
                check(&patch);
            }
        }
        patch.relax(&forbidden);
        check(&patch);
        let area: f64 = patch
            .tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| patch.at(i));
                0.5 * normal(a, b, c)[2]
            })
            .sum();
        assert!(close_to(area, 4.0, 1e-12));
    }

    // -----------------------------------------------------------------------------------------
    // Timing on big meshes

    /// A 600 × 608 torus (729,600 triangles) with 400 holes: from one triangle to 8 × 8 cells
    /// (32 edges, flat within 2%), and every eleventh hole 16 × 16 or 24 × 24 cells (64 and 96
    /// edges, curved enough to be refined and faired). In release: `cargo test --release --lib
    /// repair::holes -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn closing_hundreds_of_holes_in_a_big_mesh() {
        let (nu, nv) = (600usize, 608usize);
        let closed = torus(40.0, 15.0, nu, nv);
        let mut drop = vec![false; closed.triangle_count()];
        let mut h = 0;
        for a in 0..20 {
            for b in 0..20 {
                let (i0, j0) = (30 * a + 3, 30 * b + 3);
                let side = match h % 11 {
                    9 => 16,
                    10 => 24,
                    kind => kind,
                };
                h += 1;
                if side == 0 {
                    drop[2 * (i0 * nv + j0)] = true;
                    continue;
                }
                for i in i0..i0 + side {
                    for j in j0..j0 + side {
                        drop[2 * (i * nv + j)] = true;
                        drop[2 * (i * nv + j) + 1] = true;
                    }
                }
            }
        }
        let open = without(&closed, |t, _| drop[t]);
        let started = Instant::now();
        let edges = Edges::new(&open);
        let loops = boundary_loops(&open, &edges);
        let topology = started.elapsed().as_secs_f64();
        assert_eq!(loops.len(), 400);
        let started = Instant::now();
        let out = all(&open);
        let took = started.elapsed().as_secs_f64();
        let points = new_points(&out, &open);
        println!(
            "close all holes: {} triangles, 400 holes in {took:.3} s (edges + loops {topology:.3} \
             s): {} ({} new points)",
            open.triangle_count(),
            out.message,
            points.len()
        );
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert!(out.message.starts_with("Closed 400 holes"));
        assert_closed(&out.mesh);
        assert_areas(&out.mesh, open.triangle_count());
        assert_no_folds(&out.mesh, open.triangle_count());
        // The curved patches follow the tube (radius 15 round a circle of radius 40): a flat
        // patch over the largest holes would sag more than 0.2 mm below it.
        let worst = points
            .iter()
            .map(|&p| ((axis_distance(p) - 40.0).hypot(p[2]) - 15.0).abs())
            .fold(0.0, f64::max);
        // The flat patches sag a little below the tube, by design.
        let volume = stats(&out.mesh).volume / stats(&closed).volume - 1.0;
        println!("  new points at most {worst:.4} mm off the torus, volume off by {volume:.1e}");
        assert!(points.len() > 1000 && worst < 0.1, "{worst}");
        assert!(volume.abs() < 1e-3);
        let started = Instant::now();
        let again = all(&out.mesh);
        println!("nothing to do: {:.3} s", started.elapsed().as_secs_f64());
        assert!(!again.changed);
        let started = Instant::now();
        let out = trivial(&open);
        println!(
            "close trivial holes: {:.3} s: {}",
            started.elapsed().as_secs_f64(),
            out.message
        );
        if !cfg!(debug_assertions) {
            assert!(took < 1.0, "close all holes took {took:.3} s");
        }
    }

    /// One very long curved loop: 300° of a 4,000-segment cylinder's wall by 20 rows, 6,706
    /// edges, cut into minimum-weight pieces, refined and faired. In release: `cargo test
    /// --release --lib repair::holes -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn closing_one_very_long_curved_loop() {
        let segments = 4000u32;
        let closed = cylinder(10.0, 20.0, segments, 40);
        let span = segments * 5 / 6;
        let open = without(&closed, |t, _| {
            let (i, j) = ((t as u32 / 2) / segments, (t as u32 / 2) % segments);
            (t as u32) < segments * 40 * 2 && (10..30).contains(&i) && j < span
        });
        let ring = &boundary_loops(&open, &Edges::new(&open))[0];
        assert_eq!(ring.sides.len() as u32, 2 * (span + 20));
        let started = Instant::now();
        let out = all(&open);
        let took = started.elapsed().as_secs_f64();
        let points = new_points(&out, &open);
        let worst = points
            .iter()
            .map(|&p| (axis_distance(p) - 10.0).abs())
            .fold(0.0, f64::max);
        println!(
            "one loop of {} edges in {took:.3} s: {}, {} new points, {worst:.4} mm off the wall \
             at worst",
            ring.sides.len(),
            out.message,
            points.len()
        );
        assert!(out.changed && out.warnings.is_empty(), "{out:?}");
        assert_closed(&out.mesh);
        assert_areas(&out.mesh, open.triangle_count());
        assert_no_folds(&out.mesh, open.triangle_count());
        assert!(close_to(
            stats(&out.mesh).volume,
            stats(&closed).volume,
            0.01
        ));
    }

    /// Close all holes on real files, with timings: `TRICKY3D_MODELS="a.stl;b.obj" cargo test
    /// --release --lib repair::holes::tests::real_models -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_models() {
        let Ok(list) = std::env::var("TRICKY3D_MODELS") else {
            return;
        };
        for path in list.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let imported = crate::formats::import_file(
                std::path::Path::new(path),
                true,
                &mut crate::progress::Progress::silent(),
            )
            .unwrap();
            for part in &imported.parts {
                let m = &part.mesh;
                let edges = Edges::new(m);
                let loops = boundary_loops(m, &edges);
                let border = (0..edges.len() as u32)
                    .filter(|&e| edges.degree(e) == 1)
                    .count();
                let non_manifold = (0..edges.len() as u32)
                    .filter(|&e| edges.degree(e) > 2)
                    .count();
                let mut sizes: Vec<usize> = loops.iter().map(|l| l.sides.len()).collect();
                sizes.sort_unstable();
                let started = Instant::now();
                let out = all(m);
                let took = started.elapsed().as_secs_f64();
                let started = Instant::now();
                let trivial_out = trivial(m);
                let trivial_took = started.elapsed().as_secs_f64();
                let after = Edges::new(&out.mesh);
                let border_after = (0..after.len() as u32)
                    .filter(|&e| after.degree(e) == 1)
                    .count();
                let non_manifold_after = (0..after.len() as u32)
                    .filter(|&e| after.degree(e) > 2)
                    .count();
                println!(
                    "{path}: {} triangles, {} holes (largest {:?}), {border} border edges, \
                     {non_manifold} non-manifold\n  close all holes {took:.3} s: {} {:?}; \
                     {border_after} border edges and {non_manifold_after} non-manifold left, {} \
                     holes\n  close trivial holes {trivial_took:.3} s: {}",
                    m.triangle_count(),
                    loops.len(),
                    &sizes[sizes.len().saturating_sub(5)..],
                    out.message,
                    out.warnings,
                    holes(&out.mesh),
                    trivial_out.message,
                );
                assert_areas(&out.mesh, m.triangle_count());
                assert!(non_manifold_after <= non_manifold);
                let again = all(&out.mesh);
                assert!(!again.changed, "second run: {}", again.message);
                // The biggest holes one by one: how each was filled and how long it took.
                let wanted: Vec<&BoundaryLoop> = loops.iter().collect();
                let around = Around::new(m, &wanted);
                let never = AtomicBool::new(false);
                let mut biggest: Vec<&BoundaryLoop> = wanted.clone();
                biggest.sort_by_key(|l| std::cmp::Reverse(l.sides.len()));
                for l in biggest.iter().take(4) {
                    let ring = Ring::new(m, l);
                    let shape = Shape::new(&ring.points);
                    let started = Instant::now();
                    let hole = fill_hole(m, &around, l, Kind::All, &never);
                    let took = started.elapsed().as_secs_f64();
                    let (points, triangles) = match &hole {
                        Hole::Closed(p) => (p.points.len(), p.triangles.len()),
                        _ => (0, 0),
                    };
                    println!(
                        "  hole of {}: flat {}, simple in its plane {}, {points} new points, \
                         {triangles} triangles, {:.4} s",
                        l.sides.len(),
                        shape.flat,
                        shape.polygon.is_some(),
                        took
                    );
                }
            }
        }
    }
}
