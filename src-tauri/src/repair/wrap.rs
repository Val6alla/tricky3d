//! Wrap part surface: keep only the outer surface of the part. Inner shells, inner faces and
//! self-intersections go, so the result is one closed, oriented, non-self-intersecting shell per
//! separate body, with every vertex on the reference tool's absolute 0.002 mm grid (`GRID`), so the
//! surface stays within a grid step of where it was. Ignores the selection.
//!
//! How (a known plan, but exact instead of perturbed, with passes instead of retries):
//! 0. `prepare` makes every shell closed and outward-facing with the other actions' cores: the
//!    exact weld along seams, flat triangles zipped into their neighbours, opposite copies
//!    cancelled, no doubles, Close all holes, Fix flipped triangles, the edges left unbalanced
//!    closed (`balance`), and sheets (closed shells without thickness) removed.
//! 1. Quick path: when no triangles intersect, only closed shells inside other shells and closed
//!    shells without volume go. A wrapped mesh takes this path, so a second run changes nothing.
//! 2. `snap_contacts` makes contacts closer than two f32 units exact (between separate shells,
//!    closer than 0.001 mm, as the reference tool joins them), then `arrangement::build` splits
//!    every triangle where others cross or touch it, exactly (no perturbation: CAD models are full
//!    of exact contacts, which must stay contacts).
//! 3. `classify`: the faces form patches (joined across edges with exactly two consistently
//!    oriented faces), and one exact ray cast per patch, from a point exactly on a face, gives the
//!    winding number just in front of it; behind it the winding number is higher by the face's
//!    coverage. A face belongs to the outer surface when exactly one of the two is zero, and is
//!    turned to face the zero side.
//! 4. `surface`: where bodies touch along an edge, the faces round it are paired so that each pair
//!    bounds one wedge of solid (their order round the edge comes from the planes' normals,
//!    exactly), and every vertex gets one copy per fan. Cavities and dust go (`without_dust`).
//! 5. `snap_round` rounds the surface to the grid as snap rounding does in the plane: an edge that
//!    passes through a cell holding a vertex is routed through that cell's centre, and each face
//!    becomes the walk through those cells with its back-and-forth parts taken out, so features
//!    smaller than a cell (the folded seams of CAD caps, slivers) collapse instead of crossing.
//!    Non-manifold edges that leaves are split, one fan per copy (`split_edges`); where that
//!    would cut the surface open, the cut is welded shut and the edge left to step 6.
//! 6. `tritri` checks the whole: closed, oriented, outward, and the pairs that still cross. Their
//!    spots are wrapped again on their own (`rewrap_spots`), which leaves the rest untouched, and
//!    what that leaves is mended on the grid (`mend_on_grid`: nudges to nearby grid points, flips,
//!    collapses, patches, each checked locally), in rounds. Faces closer together than the grid can
//!    tell apart may still cross then; their spots are wrapped once more with their new points
//!    rounded to f32 instead, the only points off the grid. If that all stalls, the whole is
//!    wrapped once more. Mending stops after a fixed number of crossing tests (`MEND_TESTS`), so
//!    densely stacked faces cannot keep it busy.
//! 7. Crossings still left are covered (`covered`): each cluster inside a small axis-aligned block,
//!    joined to the surface by wrapping the whole again, so what crossed there is inside the part.
//! 8. What is still wrong, or a part that could not be wrapped at all, is rebuilt on a grid
//!    (`rebuilt`, `levelset`): the shells holding it, from the prepared part's solid, closed,
//!    oriented and without crossings by construction; the other shells stay as they are. The
//!    warning names the grid step, below which detail may be softened. Wrap part surface never
//!    returns a result that is open, misoriented or crossing itself, unless the part has no
//!    solid to rebuild.

use std::cmp::Ordering;
use std::sync::atomic::{AtomicBool, Ordering as Atomic};
use std::time::{Duration, Instant};

use rayon::prelude::*;

use super::arrangement::{self, Arrangement, E3, Failure, Pt, exp};
use super::bvh::{self, Aabb, Bvh};
use super::exact::{self, P2, P3};
use super::holes::{self, Which};
use super::levelset;
use super::topology::{self, Edges, Link};
use super::winding;
use super::{
    Concern, Control, NEW, Origin, Outcome, RepairResult, doubles, flipped, split_edges, tritri,
};
use crate::mesh::{Mesh, Welder, fmt_count};

/// Crossing pairs per face, and in all, above which the part is rebuilt on a grid without trying
/// the exact arrangement: a part of 171k faces with 5 million pairs (30 per face) ran the
/// arrangement past 8 GB, while small parts with more than 10 per face and large ones with about 4
/// wrap exactly in seconds.
const TOO_MANY_CROSSINGS: usize = 10;
const TOO_MANY_CROSSINGS_MIN: usize = 1_000_000;
/// Crossing pairs per face, and in all (`TOO_MANY_CROSSINGS_MIN`), above which a whole pass's
/// rounded result is rebuilt on the grid instead of wrapped again spot by spot: its spots run into
/// each other, so each region is about as big as the part (7.3 million pairs on 2.5 million faces
/// gave a region of 406,000 faces, still wrapping after 90 s). Successful parts leave a few
/// hundredths of a pair per face.
const TOO_MANY_LEFT: usize = 1;

/// Why a part went straight to the grid (`rebuilt`): its own crossings, or the ones rounding a whole
/// pass left, are too many to resolve exactly.
const TOO_DENSE: &str = "The surface crosses itself too densely to wrap exactly";

/// Closed shells with less volume than this × the bounding-box diagonal³ are flat: they go.
const FLAT_SHELL: f64 = 1e-12;

/// Passes of the whole wrap at most, each over the previous one's result once the spots it left
/// could not be wrapped again locally.
const PASSES: usize = 4;

/// Faces of a patch tried for a probe point, largest first.
const PROBES: usize = 16;

/// What `classify` decides per face.
const DROP: u8 = 0;
const KEEP: u8 = 1;
const TURN: u8 = 2;

/// Ray directions for the probes, tried in order: spread over the sphere, every component nonzero
/// and no simple ratio between components, so no axis-aligned or diagonal edge lines up with them.
const DIRECTIONS: [P3; 12] = [
    [0.5381, 0.7317, 0.4187],
    [-0.6143, 0.2251, 0.7562],
    [0.1427, -0.8692, 0.4733],
    [-0.3863, -0.4651, -0.7971],
    [0.8311, -0.2237, -0.5093],
    [-0.7672, 0.6157, -0.1797],
    [0.2731, 0.3359, -0.9019],
    [-0.0953, -0.9621, -0.2557],
    [0.9137, 0.3871, 0.1239],
    [-0.4517, 0.1663, -0.8767],
    [0.6529, -0.6893, 0.3139],
    [-0.8807, -0.3359, 0.3343],
];

/// How long each stage of one run took (the timing tests print it).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Timings {
    pub prepare: Duration,
    /// Finding the triangles that cross or touch: the intersection test on the prepared mesh and
    /// the arrangement's contacts.
    pub pairs: Duration,
    /// Points along edges and segments, and the triangulation of every touched triangle.
    pub retriangulation: Duration,
    /// Winding numbers per patch.
    pub classification: Duration,
    /// Assembling the arrangement and the outer surface.
    pub assembly: Duration,
    /// Rounding the outer surface to the grid.
    pub rounding: Duration,
    pub cleanup: Duration,
    pub validation: Duration,
}

pub fn run(mesh: &Mesh, selection: &[u32], control: &mut Control) -> RepairResult<Outcome> {
    let _ = selection;
    wrap(mesh, control, &mut Timings::default())
}

/// `run`, recording how long each stage took.
pub(crate) fn wrap(
    mesh: &Mesh,
    control: &mut Control,
    timings: &mut Timings,
) -> RepairResult<Outcome> {
    let clock = Instant::now();
    control.stage("Closing and orienting the shells");
    let prepared = prepare(mesh, &mut control.sub(0.0, 0.1))?;
    timings.prepare = clock.elapsed();
    // Holes the preparation could not close stay only where the part keeps its prepared surface:
    // a wrapped result is closed.
    let warnings = holes_left(prepared.open_holes);
    let holes = warnings.len();
    let clock = Instant::now();
    control.stage("Finding intersections");
    let pairs = tritri::intersecting_pairs(&prepared.mesh, &mut control.sub(0.1, 0.15))?;
    timings.pairs = clock.elapsed();
    // Far more crossings than faces, by the million, would take the arrangement more memory than
    // a machine has.
    if pairs.len() > TOO_MANY_CROSSINGS * prepared.mesh.triangle_count()
        && pairs.len() > TOO_MANY_CROSSINGS_MIN
    {
        let whole = rebuilt(
            mesh,
            &prepared.mesh,
            &prepared.origin,
            &[],
            &prepared.mesh,
            TOO_DENSE,
            control,
        )?;
        if let Some(out) = whole {
            return Ok(out);
        }
    }
    // Shells that touch along an edge, or holes the preparation could not close, take the whole
    // path even without crossings: it pairs the faces round such edges and closes the rest.
    let shape = shape_check(&prepared.mesh);
    if pairs.is_empty() && shape.open == 0 && shape.misoriented == 0 {
        let clock = Instant::now();
        control.stage("Removing inner shells");
        let keep = inner_or_flat_shells(&prepared.mesh, &mut control.sub(0.15, 1.0))?;
        timings.cleanup = clock.elapsed();
        let out = match keep {
            None if !prepared.changed => Outcome {
                warnings,
                ..Outcome::unchanged(mesh, "Nothing to wrap.")
            },
            None => finish(mesh, prepared.mesh, prepared.origin, warnings),
            Some(keep) => {
                let (out, map) = topology::keep_triangles(&prepared.mesh, &keep);
                finish(mesh, out, follow(&prepared.origin, &map), warnings)
            }
        };
        return Ok(about(out, holes));
    }
    drop(pairs);
    // The first pass wraps the whole. Where rounding to the grid made faces cross, or collapses
    // left an edge that is not closed, the spots round them are wrapped again on their own,
    // exactly, which resolves them or leaves fewer. Should that stall, the whole is wrapped once
    // more and the spots again.
    let mut wrapped: Option<Wrapped> = None;
    // The closed and oriented whole pass that left the fewest crossings: the fallback should the
    // spots' leave something wrong.
    let mut whole: Option<Wrapped> = None;
    let mut failure = None;
    for pass in 0..PASSES {
        let start = 0.15 + 0.85 * (1.0 - 0.5f64.powi(pass as i32));
        let end = 0.15 + 0.85 * (1.0 - 0.5f64.powi(pass as i32 + 1));
        let input = wrapped.as_ref().map_or(&prepared.mesh, |w| &w.0);
        let (out, map, problems, spots) =
            match surround(input, &mut control.sub(start, end), timings)? {
                Ok(result) => result,
                Err(why) => {
                    failure = Some(why);
                    break;
                }
            };
        let origin = match &wrapped {
            Some(w) => follow(&w.1, &map),
            None => follow(&prepared.origin, &map),
        };
        if problems.intersecting > TOO_MANY_LEFT * out.triangle_count()
            && problems.intersecting > TOO_MANY_CROSSINGS_MIN
        {
            let whole = rebuilt(
                mesh,
                &prepared.mesh,
                &prepared.origin,
                &[],
                &prepared.mesh,
                TOO_DENSE,
                control,
            )?;
            if let Some(out) = whole {
                return Ok(out);
            }
        }
        let result = (out, origin, problems, spots);
        if problems.acceptable()
            && whole
                .as_ref()
                .is_none_or(|w| problems.intersecting < w.2.intersecting)
        {
            whole = Some(result.clone());
        }
        let clock = Instant::now();
        let resolved = resolve_spots(result, control)?;
        timings.cleanup += clock.elapsed();
        // A later pass only stays when, its spots resolved, it leaves fewer faces on edges that
        // are not closed (a result with any is never returned), or as many and fewer spots; when
        // it does not, another would do the same.
        if wrapped
            .as_ref()
            .is_none_or(|w| spot_count(&resolved.3) < spot_count(&w.3))
        {
            wrapped = Some(resolved);
        } else {
            break;
        }
        if wrapped.as_ref().is_none_or(|w| w.3.is_empty()) {
            break;
        }
    }
    // Spots wrapped again were only checked where they changed: the whole gets its shape checked,
    // after any dust they left goes. Only a closed, oriented result is returned.
    let unchecked = match (&wrapped, &whole) {
        (Some(w), Some(base)) => w.0.indices != base.0.indices,
        (Some(_), None) => true,
        _ => false,
    };
    if unchecked && let Some(w) = wrapped.take() {
        let clock = Instant::now();
        let w = settled(w, control)?;
        timings.validation += clock.elapsed();
        let checked = w.2;
        let better = whole
            .as_ref()
            .is_none_or(|w| checked.intersecting <= w.2.intersecting);
        if checked.acceptable() && better {
            wrapped = Some(w);
        } else {
            if whole.is_none() {
                failure.get_or_insert(if checked.open > 0 {
                    "the result was not closed"
                } else {
                    "the result was not consistently oriented"
                });
            }
            wrapped = whole;
        }
    }
    // Crossings the exact wrap could not resolve: covered with small blocks.
    let mut notes = Vec::new();
    if let Some(w) = &wrapped
        && w.2.intersecting > 0
        && let Some((better, size)) = covered(w, control, timings)?
    {
        wrapped = Some(better);
        notes.push(cover_note(size));
    }
    // What is still not right, or no wrap at all: the shells it is in are rebuilt on a grid,
    // which cannot fail.
    if let Some((out, origin, problems, spots)) = &wrapped
        && problems.intersecting > 0
    {
        let bad: Vec<u32> = spots.iter().flat_map(|&(a, b)| [a, b]).collect();
        let why = "The exact wrap left intersections or holes";
        if let Some(mut out) = rebuilt(mesh, out, origin, &bad, &prepared.mesh, why, control)? {
            out.warnings.extend(notes);
            return Ok(out);
        }
    }
    let why = "The exact wrap failed";
    if wrapped.is_none()
        && let Some(out) = rebuilt(
            mesh,
            &prepared.mesh,
            &prepared.origin,
            &[],
            &prepared.mesh,
            why,
            control,
        )?
    {
        return Ok(out);
    }
    control.progress(1.0);
    match (wrapped, failure) {
        (Some((out, origin, problems, _)), _) => {
            let out = crate::ops::compact(&out.positions, out.indices);
            let mut warnings = problems.warnings();
            let crossing = warnings.len();
            warnings.extend(notes);
            let mut out = about(finish(mesh, out, origin, warnings), 0);
            // The note about blocks is no concern a later action could resolve.
            out.concerns.truncate(crossing);
            Ok(out)
        }
        (None, why) => {
            let why = why.unwrap_or("no pass finished");
            let kept = match prepared.unbalanced {
                Some(before) => *before,
                None => prepared,
            };
            let mut warnings = holes_left(kept.open_holes);
            let holes = warnings.len();
            warnings.push(format!(
                "Wrap part surface could not resolve the intersections ({why}), so the part keeps \
                 them."
            ));
            let out = if kept.changed {
                let shape = shape_check(&kept.mesh);
                let message = if shape.open == 0 && shape.misoriented == 0 {
                    "Closed and oriented the shells, but could not wrap the surface."
                } else {
                    "Could not wrap the surface."
                };
                Outcome {
                    message: message.into(),
                    ..finish(mesh, kept.mesh, kept.origin, warnings)
                }
            } else {
                Outcome {
                    warnings,
                    ..Outcome::unchanged(mesh, "Could not wrap the surface.")
                }
            };
            Ok(about(out, holes))
        }
    }
}

/// The outer surface of `mesh` (closed, outward-facing shells, some of which intersect): near
/// contacts snapped exact, the arrangement, the classification, the surface without cavities and
/// dust, rounded to the grid, cleanup, and the whole check. The result, per triangle the triangle
/// of `mesh` it came from, what is still wrong with it and its spots; Err when the arrangement
/// failed.
fn surround(
    mesh: &Mesh,
    control: &mut Control,
    timings: &mut Timings,
) -> RepairResult<Result<Wrapped, Failure>> {
    let clock = Instant::now();
    control.stage("Snapping near contacts");
    let (work, work_origin) = arrangement_input(mesh);
    let (work, work_origin) = match snap_contacts(&work, control)? {
        Some((snapped, map)) => {
            // The snap can leave triangles without area or repeat a vertex; the arrangement
            // takes neither.
            let (clean, more) = arrangement_input(&snapped);
            let map = follow(&map, &more);
            (clean, follow(&work_origin, &map))
        }
        None => (work, work_origin),
    };
    timings.pairs += clock.elapsed();
    control.stage("Splitting the triangles where they cross");
    let arr = match arrangement::build(&work, &mut control.sub(0.05, 0.6))? {
        Ok(arr) => arr,
        Err(why) => return Ok(Err(why)),
    };
    timings.pairs += arr.counts.time_contacts;
    timings.retriangulation += arr.counts.time_points + arr.counts.time_triangulation;
    timings.assembly += arr.counts.time_assembly;
    let clock = Instant::now();
    control.stage("Telling inside from outside");
    let decision = classify(&arr, None, &mut control.sub(0.6, 0.7))?;
    timings.classification += clock.elapsed();
    let clock = Instant::now();
    control.stage("Building the outer surface");
    let surf = surface(&arr, &decision);
    drop(arr);
    control.check()?;
    let surf = surf.without_cavities();
    control.check()?;
    timings.assembly += clock.elapsed();
    let clock = Instant::now();
    control.stage("Rounding to the grid");
    let (rounded, round_origin, _) = round_surface(&surf, &|_| false, &mut control.sub(0.7, 0.85))?;
    timings.rounding += clock.elapsed();
    let clock = Instant::now();
    let (out, keep_map) = drop_dust(rounded, &mut control.sub(0.85, 0.86))?;
    let origin: Vec<u32> = keep_map
        .iter()
        .map(|&t| work_origin[surf.source[round_origin[t as usize] as usize] as usize])
        .collect();
    timings.cleanup += clock.elapsed();
    // The whole check: even faces whose corners did not move can cross, where the exact surface
    // was degenerate (two faces over the same corners).
    let clock = Instant::now();
    control.stage("Checking the result");
    let mut spots = tritri::intersecting_pairs(&out, &mut control.sub(0.86, 1.0))?;
    let problems = Check {
        intersecting: spots.len(),
        ..shape_check(&out)
    };
    if !problems.acceptable() {
        spots.extend(unclosed_faces(&out).into_iter().map(|t| (t, t)));
        spots.sort_unstable();
    }
    timings.validation += clock.elapsed();
    Ok(Ok((out, origin, problems, spots)))
}

/// A result whose spots were wrapped again on their own, which only checked where they changed:
/// any dust they left goes, and the whole gets its shape checked.
fn settled(w: Wrapped, control: &mut Control) -> RepairResult<Wrapped> {
    let (out, origin, _, spots) = w;
    control.stage("Checking the result");
    let n = out.triangle_count();
    let (clean, keep) = drop_dust(out, &mut control.sub(0.99, 1.0))?;
    let mut kept_id = vec![u32::MAX; n];
    for (i, &t) in keep.iter().enumerate() {
        kept_id[t as usize] = i as u32;
    }
    let spots: Vec<(u32, u32)> = spots
        .iter()
        .filter_map(|&(a, b)| match (kept_id[a as usize], kept_id[b as usize]) {
            (u32::MAX, _) | (_, u32::MAX) => None,
            (a, b) => Some((a.min(b), a.max(b))),
        })
        .collect();
    let checked = Check {
        intersecting: spots.iter().filter(|&&(a, b)| a != b).count(),
        ..shape_check(&clean)
    };
    let origin = keep.iter().map(|&t| origin[t as usize]).collect();
    Ok((clean, origin, checked, spots))
}

/// Rounds of covering at most, each with blocks reaching twice as far as the last.
const COVER_ROUNDS: usize = 3;

/// How far the first blocks reach past the crossings they cover, in grid steps.
const COVER_REACH: f64 = 4.0;

/// The crossings of `w` covered: each cluster of them inside an axis-aligned block reaching
/// `COVER_REACH` grid steps past where they cross, joined to the surface by wrapping the whole
/// again. Inside a block everything is inside the part, so what crossed there goes. Where joining
/// the blocks leaves crossings of its own, those are covered by blocks reaching twice as far, for
/// `COVER_ROUNDS` rounds. The closed and oriented result with the fewest crossings, and the
/// largest block's longest side, when it has fewer than `w`.
fn covered(
    w: &Wrapped,
    control: &mut Control,
    timings: &mut Timings,
) -> RepairResult<Option<(Wrapped, f64)>> {
    let mut best: Option<(Wrapped, f64)> = None;
    let mut size = 0.0f64;
    let mut reach = COVER_REACH * GRID;
    for _ in 0..COVER_ROUNDS {
        let current = best.as_ref().map_or(w, |b| &b.0);
        if current.2.intersecting == 0 {
            break;
        }
        control.stage("Covering the crossings left");
        let pairs: Vec<(u32, u32)> = current.3.iter().copied().filter(|&(a, b)| a != b).collect();
        let blocks: Vec<Mesh> = cover_boxes(&current.0, &pairs, reach)
            .iter()
            .map(|b| {
                size = size.max((0..3).map(|k| b[1][k] - b[0][k]).fold(0.0, f64::max));
                crate::mesh::make_box(b[0].map(|x| x as f32), b[1].map(|x| x as f32))
            })
            .collect();
        reach *= 2.0;
        let mut parts: Vec<&Mesh> = vec![&current.0];
        parts.extend(blocks.iter());
        let input = crate::ops::concat(&parts).map_err(super::RepairError::Failed)?;
        let mut origin = current.1.clone();
        origin.resize(input.triangle_count(), NEW);
        let Ok((out, map, problems, spots)) = surround(&input, control, timings)? else {
            break;
        };
        let result = (out, follow(&origin, &map), problems, spots);
        let result = settled(resolve_spots(result, control)?, control)?;
        if result.2.acceptable()
            && result.2.intersecting < best.as_ref().map_or(w, |b| &b.0).2.intersecting
        {
            best = Some((result, size));
        } else {
            break;
        }
    }
    Ok(best)
}

/// Boxes round the crossings of `mesh` (`pairs` of faces): per pair, the box round where they
/// cross (`crossing_box`), grown by `reach`, and boxes that touch merged into the box round both,
/// until none touch.
fn cover_boxes(mesh: &Mesh, pairs: &[(u32, u32)], reach: f64) -> Vec<Aabb> {
    let mut boxes: Vec<Aabb> = pairs
        .iter()
        .map(|&(a, b)| {
            grown(
                crossing_box(exact::corners(mesh, a), exact::corners(mesh, b)),
                reach,
            )
        })
        .collect();
    loop {
        let tree = Bvh::new(&boxes);
        let mut parent: Vec<u32> = (0..boxes.len() as u32).collect();
        let mut merged = false;
        tree.self_pairs(|a, b| {
            if bvh::overlap(&boxes[a as usize], &boxes[b as usize]) {
                join(&mut parent, a, b);
                merged = true;
            }
        });
        if !merged {
            return boxes;
        }
        let mut united: std::collections::BTreeMap<u32, Aabb> = Default::default();
        for i in 0..boxes.len() as u32 {
            let r = root(&mut parent, i);
            bvh::grow(united.entry(r).or_insert(bvh::EMPTY), &boxes[i as usize]);
        }
        boxes = united.into_values().collect();
    }
}

/// A box round where triangles `p` and `q` cross: round the points where the sides of each pass
/// through the other (their crossing is the hull of those points), or, when they lie in one plane
/// or rounding found no such point, the overlap of their boxes.
fn crossing_box(p: [P3; 3], q: [P3; 3]) -> Aabb {
    let mut b = bvh::EMPTY;
    for (s, t) in [(p, q), (q, p)] {
        for k in 0..3 {
            let (o, e) = (s[k], s[(k + 1) % 3]);
            let dir = exact::sub(e, o);
            if let Some(f) = levelset::segment_hit(o, dir, t[0], t[1], t[2]) {
                bvh::include(&mut b, exact::add(o, exact::scale(dir, f)));
            }
        }
    }
    if b[0][0] <= b[1][0] {
        return b;
    }
    let (bp, bq) = (box_of(&p), box_of(&q));
    [
        [0, 1, 2].map(|k| bp[0][k].max(bq[0][k])),
        [0, 1, 2].map(|k| bp[1][k].min(bq[1][k])),
    ]
}

/// The note for a result whose crossings were covered with blocks up to `size` mm across.
fn cover_note(size: f64) -> String {
    let size = format!("{size:.3}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string();
    format!(
        "The exact wrap left a few spots where the surface crossed itself; they were filled with \
         small blocks up to {size} mm across, so detail below {size} mm may be softened there."
    )
}

/// How many of `spots` are faces on edges that are not closed, (t, t), and how many there are.
fn spot_count(spots: &[(u32, u32)]) -> (usize, usize) {
    (spots.iter().filter(|&&(a, b)| a == b).count(), spots.len())
}

/// The faces on edges that do not have exactly two sides running opposite ways (sorted).
fn unclosed_faces(mesh: &Mesh) -> Vec<u32> {
    let edges = Edges::new(mesh);
    let mut out: Vec<u32> = (0..edges.len() as u32)
        .filter(|&e| edges.degree(e) != 2 || !topology::is_consistent(mesh, &edges, e))
        .flat_map(|e| edges.sides(e).iter().map(|&s| s / 3))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The outcome for a wrapped mesh. Inner triangles are input triangles that left no piece behind.
fn finish(input: &Mesh, mesh: Mesh, origin: Vec<u32>, warnings: Vec<String>) -> Outcome {
    let mut used = vec![false; input.triangle_count()];
    for &t in &origin {
        if t != NEW {
            used[t as usize] = true;
        }
    }
    let removed = used.iter().filter(|&&u| !u).count();
    let shells = count_of(crate::mesh::stats(&mesh).shells as usize, "shell", "shells");
    let message = if removed > 0 {
        format!(
            "Wrapped the surface: removed {}, {shells} left.",
            count_of(removed, "inner triangle", "inner triangles")
        )
    } else {
        format!("Wrapped the surface: {shells} left.")
    };
    Outcome {
        mesh,
        changed: true,
        origin: Origin::Map(origin),
        message,
        warnings,
        highlight: None,
        select: None,
        concerns: Vec::new(),
    }
}

/// `out` with what its warnings are about: the first `holes` about holes, the rest about
/// intersections.
fn about(mut out: Outcome, holes: usize) -> Outcome {
    out.concerns = (0..out.warnings.len())
        .map(|i| {
            (
                i,
                if i < holes {
                    Concern::Holes
                } else {
                    Concern::Crossings
                },
            )
        })
        .collect();
    out
}

/// The warning about holes the preparation could not close, if there are any.
fn holes_left(open_holes: usize) -> Vec<String> {
    if open_holes == 0 {
        return Vec::new();
    }
    vec![format!(
        "{} could not be closed.",
        count_of(open_holes, "hole", "holes")
    )]
}

/// "1 shell", "1,204 shells".
fn count_of(n: usize, one: &str, many: &str) -> String {
    format!(
        "{} {}",
        fmt_count(n as u64),
        if n == 1 { one } else { many }
    )
}

/// `origin` carried through a step whose own map gives, per output triangle, its input triangle.
fn follow(origin: &[u32], map: &[u32]) -> Vec<u32> {
    map.iter()
        .map(|&t| if t == NEW { NEW } else { origin[t as usize] })
        .collect()
}

/// True when the triangle's three vertex indices differ.
fn distinct(t: [u32; 3]) -> bool {
    t[0] != t[1] && t[1] != t[2] && t[2] != t[0]
}

// ---------------------------------------------------------------------------------------------
// 0. Preprocessing

struct Prepared {
    mesh: Mesh,
    /// Input triangle of each triangle, NEW for hole patches.
    origin: Vec<u32>,
    changed: bool,
    /// Holes that Close all holes could not close.
    open_holes: usize,
    /// The preparation as it was before `balance` patched it, when it did: what the part keeps if
    /// it cannot be wrapped. The patches only make the winding numbers defined; across an edge
    /// whose two sides run the same way (a shell that cannot be oriented) they are single
    /// triangles on non-manifold edges, which no result may keep.
    unbalanced: Option<Box<Prepared>>,
}

fn prepare(mesh: &Mesh, control: &mut Control) -> RepairResult<Prepared> {
    let mut changed = false;
    let mut current = match weld_seams(mesh) {
        Some(welded) => {
            changed = true;
            welded
        }
        None => mesh.clone(),
    };
    let mut origin: Vec<u32> = (0..mesh.triangle_count() as u32).collect();
    if let Some((kept, map)) = without_repeated(&current) {
        (current, origin) = (kept, follow(&origin, &map));
        changed = true;
    }
    for step in [zip_collinear, cancel_opposite] {
        control.check()?;
        if let Some((next, map)) = step(&current) {
            (current, origin) = (next, follow(&origin, &map));
            changed = true;
        }
    }
    control.check()?;
    // Bodies glued face to face lost their shared faces: join them along the seam that left.
    if let Some(welded) = weld_seams(&current) {
        current = welded;
        changed = true;
    }
    let (single, map, removed) = doubles::remove_doubles(&current);
    if removed > 0 {
        (current, origin) = (single, follow(&origin, &map));
        changed = true;
    }
    control.progress(0.3);
    let edges = Edges::new(&current);
    let open = (0..edges.len() as u32).any(|e| edges.degree(e) == 1);
    drop(edges);
    let mut open_holes = 0;
    if open {
        let filled = holes::fill(&current, Which::All, &mut control.sub(0.3, 0.8))?;
        open_holes = filled.failed;
        if filled.added > 0 {
            (current, origin) = (filled.mesh, follow(&origin, &filled.origin));
            changed = true;
        }
        if let Some((kept, map)) = without_repeated(&current) {
            (current, origin) = (kept, follow(&origin, &map));
        }
    }
    let (oriented, flips) = flipped::orient(&current, &mut control.sub(0.8, 1.0))?;
    if flips > 0 {
        current = oriented;
        changed = true;
    }
    let mut unbalanced = None;
    if let Some((balanced, map, unfilled)) = balance(&current) {
        let next = follow(&origin, &map);
        unbalanced = Some(Box::new(Prepared {
            mesh: std::mem::replace(&mut current, balanced),
            origin: std::mem::replace(&mut origin, next),
            changed,
            open_holes,
            unbalanced: None,
        }));
        changed = true;
        open_holes = unfilled;
    }
    if let Some((kept, map)) = without_sheets(&current) {
        (current, origin) = (kept, follow(&origin, &map));
        changed = true;
    }
    Ok(Prepared {
        mesh: current,
        origin,
        changed,
        open_holes,
        unbalanced,
    })
}

/// Closed shells thinner on average than this many f32 units of the largest coordinate, or than
/// this fraction of the bounding box's diagonal, are sheets: an open surface that Close all holes
/// closed with a copy of itself, two faces of a part pressed together, or dust. No printer can
/// build them.
const SHEET_ULPS: f64 = 64.0;
const SHEET_SIZE: f64 = 1e-4;

/// The sheet thickness for these points (`SHEET_ULPS`, `SHEET_SIZE`).
fn sheet_size(points: impl Iterator<Item = P3>) -> f64 {
    let mut b = bvh::EMPTY;
    for p in points {
        bvh::include(&mut b, p);
    }
    if b[0][0] > b[1][0] {
        return 0.0;
    }
    let largest = (0..3).fold(0.0f64, |m, k| m.max(b[0][k].abs()).max(b[1][k].abs()));
    let diagonal = exact::length(exact::sub(b[1], b[0]));
    (SHEET_ULPS * ulp_f32(largest)).max(SHEET_SIZE * diagonal)
}

/// Without sheets: closed shells (joined through edges, every edge balanced) whose mean thickness,
/// twice the volume over the area, is below `sheet_size`. Their two layers lie within float noise
/// of each other, so they would cross each other everywhere at grazing angles and leave dust that
/// rounding breaks; they hold nothing printable. The mesh and its origin map; None when there are
/// none.
fn without_sheets(mesh: &Mesh) -> Option<(Mesh, Vec<u32>)> {
    let edges = Edges::new(mesh);
    let (shell_of, shells) = topology::components(mesh, &edges, Link::Edge);
    let mut closed = vec![true; shells];
    for e in 0..edges.len() as u32 {
        let [lo, _] = edges.ends(e);
        let sides = edges.sides(e);
        let up = sides
            .iter()
            .filter(|&&s| topology::side_ends(mesh, s).0 == lo)
            .count();
        if up * 2 != sides.len() {
            closed[shell_of[sides[0] as usize / 3] as usize] = false;
        }
    }
    drop(edges);
    let (mut volume, mut area) = (vec![0.0f64; shells], vec![0.0f64; shells]);
    let mut apex: Vec<Option<P3>> = vec![None; shells];
    for (t, tri) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        let s = shell_of[t] as usize;
        let p = tri.map(|v| mesh.point(v));
        let o = *apex[s].get_or_insert(p[0]);
        volume[s] += volume6(o, p) / 6.0;
        area[s] += exact::length(exact::normal(p[0], p[1], p[2])) / 2.0;
    }
    let thin = sheet_size((0..mesh.vertex_count() as u32).map(|v| mesh.point(v)));
    let sheet: Vec<bool> = (0..shells)
        .map(|s| closed[s] && 2.0 * volume[s].abs() < thin * area[s])
        .collect();
    if !sheet.contains(&true) {
        return None;
    }
    let keep: Vec<bool> = shell_of.iter().map(|&s| !sheet[s as usize]).collect();
    Some(topology::keep_triangles(mesh, &keep))
}

/// A closed shell that touches a shell holding this many times its volume is a crumb, unless it is
/// at least `CRUMB_SIZE` of the bounding box's diagonal across (as a cube of its volume).
const CRUMB: f64 = 1e4;
const CRUMB_SIZE: f64 = 1e-2;

/// Which triangles stay when the shells (joined through edges) that hold no printable material go:
/// closed shells enclosing no positive volume (cavities inside the union, or dust that rounding
/// turned over), sheets (mean thickness, twice the volume over the area, below `sheet_size`), and
/// crumbs: small closed shells sharing a corner position with a shell that holds `CRUMB` times
/// their volume, the dust left where bodies meet along a pinch. None when every shell stays.
fn without_dust(tris: &[[u32; 3]], point: &impl Fn(u32) -> P3, count: usize) -> Option<Vec<bool>> {
    let index = index_mesh(count, tris);
    let edges = Edges::new(&index);
    let (shell_of, shells) = topology::components(&index, &edges, Link::Edge);
    let mut closed = vec![true; shells];
    for e in 0..edges.len() as u32 {
        if !topology::is_consistent(&index, &edges, e) {
            for &side in edges.sides(e) {
                closed[shell_of[side as usize / 3] as usize] = false;
            }
        }
    }
    drop((edges, index));
    let (mut volume, mut area) = (vec![0.0f64; shells], vec![0.0f64; shells]);
    let mut apex: Vec<Option<P3>> = vec![None; shells];
    let mut bounds = bvh::EMPTY;
    for (t, tri) in tris.iter().enumerate() {
        let s = shell_of[t] as usize;
        let p = tri.map(point);
        let o = *apex[s].get_or_insert(p[0]);
        volume[s] += volume6(o, p) / 6.0;
        area[s] += exact::length(exact::normal(p[0], p[1], p[2])) / 2.0;
        for q in p {
            bvh::include(&mut bounds, q);
        }
    }
    if tris.is_empty() {
        return None;
    }
    let sheet = sheet_size(bounds.into_iter());
    let crumb = (CRUMB_SIZE * exact::length(exact::sub(bounds[1], bounds[0]))).powi(3);
    // Per shell, the largest volume among the shells it shares a corner position with.
    let mut owner: std::collections::HashMap<[u64; 3], u32> = Default::default();
    let mut beside = vec![0.0f64; shells];
    for (t, tri) in tris.iter().enumerate() {
        let s = shell_of[t];
        for &v in tri {
            let o = *owner.entry(point(v).map(f64::to_bits)).or_insert(s);
            if o != s {
                let (a, b) = (s as usize, o as usize);
                beside[a] = beside[a].max(volume[b]);
                beside[b] = beside[b].max(volume[a]);
            }
        }
    }
    let gone: Vec<bool> = (0..shells)
        .map(|s| {
            closed[s]
                && (volume[s] <= 0.0
                    || 2.0 * volume[s] < sheet * area[s]
                    || (volume[s] < crumb && beside[s] >= CRUMB * volume[s]))
        })
        .collect();
    if !gone.contains(&true) {
        return None;
    }
    Some(shell_of.iter().map(|&s| !gone[s as usize]).collect())
}

/// Closes what Close all holes leaves open: every edge whose sides do not balance (more running one
/// way than the other: a border edge, or a junction where an open piece meets a closed one along a
/// non-manifold edge) gets the missing sides from patches, so the winding numbers the
/// classification needs are well defined. The surplus sides, one per unit of imbalance and in the
/// surplus direction, form closed loops (at every vertex as many arrive as leave); each loop is
/// filled like a hole (`fill_loop`). Where an open tube meets the rim of a closed shell's cap, the
/// patch is the cap again facing the other way, so the two cancel and the bodies join. The mesh
/// (patches appended, new vertices at the end), its origin map (NEW for patches) and how many
/// loops could not be filled; None when every edge balances.
fn balance(mesh: &Mesh) -> Option<(Mesh, Vec<u32>, usize)> {
    let edges = Edges::new(mesh);
    let mut surplus: Vec<(u32, u32)> = Vec::new();
    for e in 0..edges.len() as u32 {
        let [lo, hi] = edges.ends(e);
        let up = edges
            .sides(e)
            .iter()
            .filter(|&&s| topology::side_ends(mesh, s).0 == lo)
            .count();
        let down = edges.degree(e) - up;
        surplus.extend(std::iter::repeat_n((lo, hi), up.saturating_sub(down)));
        surplus.extend(std::iter::repeat_n((hi, lo), down.saturating_sub(up)));
    }
    drop(edges);
    if surplus.is_empty() {
        return None;
    }
    let loops = loops_of(surplus, mesh.vertex_count());
    let mut extra: Vec<f32> = Vec::new();
    let mut patches: Vec<[u32; 3]> = Vec::new();
    let mut unfilled = 0;
    for border in &loops {
        match fill_loop(mesh, border, &mut extra) {
            Some(more) => patches.extend(more),
            None => unfilled += 1,
        }
    }
    let mut positions = mesh.positions.clone();
    positions.extend_from_slice(&extra);
    let mut indices = mesh.indices.clone();
    indices.extend(patches.iter().flatten());
    let mut origin: Vec<u32> = (0..mesh.triangle_count() as u32).collect();
    origin.extend(std::iter::repeat_n(NEW, patches.len()));
    Some((Mesh { positions, indices }, origin, unfilled))
}

/// Directed sides (from, to) chained into simple loops, each as its vertices in the order the
/// sides run: at every vertex as many sides must arrive as leave. A walk that comes back to a
/// vertex it passed closes a loop there and goes on. Sides that cannot close stay out.
fn loops_of(mut sides: Vec<(u32, u32)>, count: usize) -> Vec<Vec<u32>> {
    sides.sort_unstable();
    let mut used = vec![false; sides.len()];
    let mut next_out = vec![0usize; count + 1];
    for &(a, _) in &sides {
        next_out[a as usize + 1] += 1;
    }
    for v in 0..count {
        next_out[v + 1] += next_out[v];
    }
    let end_of = |v: u32| sides.partition_point(|s| s.0 <= v);
    let mut loops: Vec<Vec<u32>> = Vec::new();
    let mut walk: Vec<u32> = Vec::new();
    let mut place = vec![u32::MAX; count];
    for first in 0..sides.len() {
        if used[first] {
            continue;
        }
        used[first] = true;
        walk.clear();
        walk.push(sides[first].0);
        place[sides[first].0 as usize] = 0;
        let mut at = sides[first].1;
        loop {
            // A vertex the walk visited before closes a simple loop.
            let p = place[at as usize];
            if p != u32::MAX {
                let closed: Vec<u32> = walk.drain(p as usize..).collect();
                for &v in &closed {
                    place[v as usize] = u32::MAX;
                }
                loops.push(closed);
                if walk.is_empty() {
                    break;
                }
            }
            place[at as usize] = walk.len() as u32;
            walk.push(at);
            // The next unused side leaving `at`.
            let mut k = next_out[at as usize];
            let stop = end_of(at);
            while k < stop && used[k] {
                k += 1;
            }
            next_out[at as usize] = k;
            if k >= stop {
                break;
            }
            used[k] = true;
            at = sides[k].1;
        }
        for &v in &walk {
            place[v as usize] = u32::MAX;
        }
    }
    loops
}

/// A triangulation of the loop `border` (vertices in the order its surplus sides run; the patch
/// uses each edge the other way): ear clipping in the plane the loop spans most (exact predicates,
/// best-shaped ear first), or, when that projection folds, a fan round a new vertex at the loop's
/// centroid (appended to `extra`). None for a loop of fewer than three vertices.
fn fill_loop(mesh: &Mesh, border: &[u32], extra: &mut Vec<f32>) -> Option<Vec<[u32; 3]>> {
    let next = (mesh.vertex_count() + extra.len() / 3) as u32;
    let (faces, center) = fill_loop_with(&|v| mesh.point(v), next, border)?;
    if let Some(c) = center {
        extra.extend_from_slice(&c);
    }
    Some(faces)
}

/// `fill_loop` over points `point`: the faces, and the position of the fan's new vertex (which
/// gets the id `next`) when there is one.
#[allow(clippy::type_complexity)]
fn fill_loop_with(
    point: &impl Fn(u32) -> P3,
    next: u32,
    border: &[u32],
) -> Option<(Vec<[u32; 3]>, Option<[f32; 3]>)> {
    if border.len() < 3 {
        return None;
    }
    // The polygon runs the loop backwards.
    let poly: Vec<u32> = border.iter().rev().copied().collect();
    let pts: Vec<P3> = poly.iter().map(|&v| point(v)).collect();
    if poly.len() == 3 {
        return Some((vec![[poly[0], poly[1], poly[2]]], None));
    }
    // Newell's normal picks the projection.
    let mut normal = [0.0f64; 3];
    for i in 0..pts.len() {
        let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
        normal[0] += (p[1] - q[1]) * (p[2] + q[2]);
        normal[1] += (p[2] - q[2]) * (p[0] + q[0]);
        normal[2] += (p[0] - q[0]) * (p[1] + q[1]);
    }
    let axis = (0..3)
        .max_by(|&x, &y| normal[x].abs().total_cmp(&normal[y].abs()))
        .unwrap_or(2);
    let flat: Vec<[f64; 2]> = pts.iter().map(|&p| exact::project(p, axis)).collect();
    let turn = if normal[axis] > 0.0 { 1 } else { -1 };
    if let Some(out) = ear_clip(&poly, &flat, turn) {
        return Some((out, None));
    }
    let c = pts
        .iter()
        .fold([0.0; 3], |acc, &p| exact::add(acc, p))
        .map(|x| (x / pts.len() as f64) as f32);
    Some((
        (0..poly.len())
            .map(|i| [poly[i], poly[(i + 1) % poly.len()], next])
            .collect(),
        Some(c),
    ))
}

/// Ear clipping of the polygon `poly` (vertex ids) whose projected points `flat` turn the way of
/// `turn` (+1 counter-clockwise): repeatedly cuts off the ear whose smallest angle is largest, an
/// ear being a strictly convex corner with no other polygon vertex inside or on its triangle (exact
/// orient2d). None when the projection folds and no ear is left.
fn ear_clip(poly: &[u32], flat: &[[f64; 2]], turn: i32) -> Option<Vec<[u32; 3]>> {
    let n = poly.len();
    let mut prev: Vec<usize> = (0..n).map(|i| (i + n - 1) % n).collect();
    let mut next: Vec<usize> = (0..n).map(|i| (i + 1) % n).collect();
    let mut live = vec![true; n];
    let mut count = n;
    // The shape of the ear at b, or None when b is no ear.
    let ear = |b: usize, prev: &[usize], next: &[usize], live: &[bool]| -> Option<f64> {
        let (a, c) = (prev[b], next[b]);
        let (pa, pb, pc) = (flat[a], flat[b], flat[c]);
        if exact::orient2d(pa, pb, pc) != turn {
            return None;
        }
        let blocked = (0..n).any(|q| {
            live[q]
                && q != a
                && q != b
                && q != c
                && exact::orient2d(pa, pb, flat[q]) * turn >= 0
                && exact::orient2d(pb, pc, flat[q]) * turn >= 0
                && exact::orient2d(pc, pa, flat[q]) * turn >= 0
        });
        (!blocked).then(|| smallest_angle(pa, pb, pc))
    };
    let mut shape: Vec<Option<f64>> = (0..n).map(|b| ear(b, &prev, &next, &live)).collect();
    let mut out = Vec::with_capacity(n.saturating_sub(2));
    while count > 3 {
        let best = |shape: &[Option<f64>]| {
            (0..n)
                .filter(|&b| live[b])
                .filter_map(|b| shape[b].map(|s| (s, b)))
                .max_by(|x, y| x.0.total_cmp(&y.0).then(y.1.cmp(&x.1)))
        };
        // Clipping an ear can free others that only its corner blocked: when no ear is known,
        // every corner is looked at again.
        let found = match best(&shape) {
            Some(x) => Some(x),
            None => {
                for b in 0..n {
                    if live[b] {
                        shape[b] = ear(b, &prev, &next, &live);
                    }
                }
                best(&shape)
            }
        };
        let (_, b) = found?;
        let (a, c) = (prev[b], next[b]);
        out.push([poly[a], poly[b], poly[c]]);
        live[b] = false;
        count -= 1;
        next[a] = c;
        prev[c] = a;
        shape[a] = ear(a, &prev, &next, &live);
        shape[c] = ear(c, &prev, &next, &live);
    }
    let b = (0..n).find(|&b| live[b])?;
    let (a, c) = (prev[b], next[b]);
    if exact::orient2d(flat[a], flat[b], flat[c]) != turn {
        return None;
    }
    out.push([poly[a], poly[b], poly[c]]);
    Some(out)
}

/// The sine of a triangle's smallest angle, as a shape measure (0 for a flat one).
fn smallest_angle(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    let d = |p: [f64; 2], q: [f64; 2]| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt();
    let (ab, bc, ca) = (d(a, b), d(b, c), d(c, a));
    let area2 = ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs();
    // The smallest angle is opposite the shortest side: sin = 2 area / (product of the other two).
    let (p, q) = if ab <= bc && ab <= ca {
        (bc, ca)
    } else if bc <= ca {
        (ab, ca)
    } else {
        (ab, bc)
    };
    if ab.max(bc).max(ca) == 0.0 {
        0.0
    } else {
        area2 / (p * q).max(f64::MIN_POSITIVE)
    }
}

/// The mesh without triangles that repeat a vertex index, and its origin map; None when there are
/// none. Such a triangle adds one side each way to its one edge, so the rest stays as closed as it
/// was.
fn without_repeated(mesh: &Mesh) -> Option<(Mesh, Vec<u32>)> {
    let keep: Vec<bool> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|&t| distinct(t))
        .collect();
    if !keep.contains(&false) {
        return None;
    }
    Some(topology::keep_triangles(mesh, &keep))
}

/// The exact weld along seams: vertices on border edges with equal coordinates become the first
/// of them (vertex list and triangle order kept). That closes cracks and joins a triangle soup, and
/// joins bodies whose glued faces cancelled. Coincident vertices off the borders stay apart on
/// purpose: there bodies merely touch (by a face, an edge or a corner), or a wrapped mesh split an
/// edge so it keeps two sides, and merging them would make edges non-manifold. None when nothing
/// merges.
fn weld_seams(mesh: &Mesh) -> Option<Mesh> {
    let edges = Edges::new(mesh);
    let mut border = vec![false; mesh.vertex_count()];
    for e in 0..edges.len() as u32 {
        if edges.degree(e) == 1 {
            for v in edges.ends(e) {
                border[v as usize] = true;
            }
        }
    }
    drop(edges);
    let mut welder = Welder::new(mesh.indices.len());
    let mut first: Vec<u32> = Vec::new();
    let mut to: Vec<u32> = (0..mesh.vertex_count() as u32).collect();
    for v in 0..mesh.vertex_count() {
        if border[v] {
            let g = welder.vertex(mesh.vertex(v as u32)) as usize;
            if g == first.len() {
                first.push(v as u32);
            }
            to[v] = first[g];
        }
    }
    if mesh.indices.iter().all(|&v| to[v as usize] == v) {
        return None;
    }
    Some(Mesh {
        positions: mesh.positions.clone(),
        indices: mesh.indices.iter().map(|&v| to[v as usize]).collect(),
    })
}

/// The arrangement's input: every group of vertices with equal coordinates becomes its first
/// vertex (so corners are shared exactly when their coordinates are), and triangles that lose a
/// corner that way or have no area go (they enclose nothing; the arrangement mends the cracks they
/// leave, as their corners lie on their neighbours' edges). The mesh and its origin map.
fn arrangement_input(mesh: &Mesh) -> (Mesh, Vec<u32>) {
    let mut welder = Welder::new(mesh.indices.len());
    let mut first: Vec<u32> = Vec::new();
    let to: Vec<u32> = (0..mesh.vertex_count() as u32)
        .map(|v| {
            let g = welder.vertex(mesh.vertex(v)) as usize;
            if g == first.len() {
                first.push(v);
            }
            first[g]
        })
        .collect();
    let mut tris: Vec<[u32; 3]> = Vec::with_capacity(mesh.triangle_count());
    let mut origin = Vec::with_capacity(mesh.triangle_count());
    for (t, tri) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        let moved = tri.map(|v| to[v as usize]);
        let [a, b, c] = moved.map(|v| mesh.point(v));
        if distinct(moved) && !exact::collinear(a, b, c) {
            tris.push(moved);
            origin.push(t as u32);
        }
    }
    // Triangles over the same corners facing opposite ways enclose nothing: they go in pairs, which
    // keeps every edge balanced. (Copies facing one way stay: they are layers the classification
    // counts.)
    let mut keys: Vec<([u32; 3], bool, u32)> = tris
        .par_iter()
        .enumerate()
        .map(|(t, &tri)| {
            let (key, even) = sorted(tri);
            (key, even, t as u32)
        })
        .collect();
    keys.par_sort_unstable();
    let mut keep = vec![true; tris.len()];
    for group in keys.chunk_by(|a, b| a.0 == b.0) {
        if group.len() < 2 {
            continue;
        }
        let odd = group.partition_point(|k| !k.1);
        let pairs = odd.min(group.len() - odd);
        for k in group[odd - pairs..odd]
            .iter()
            .chain(&group[group.len() - pairs..])
        {
            keep[k.2 as usize] = false;
        }
    }
    let mut indices = Vec::with_capacity(tris.len() * 3);
    let mut kept_origin = Vec::with_capacity(tris.len());
    for (t, tri) in tris.iter().enumerate() {
        if keep[t] {
            indices.extend_from_slice(tri);
            kept_origin.push(origin[t]);
        }
    }
    (
        Mesh {
            positions: mesh.positions.clone(),
            indices,
        },
        kept_origin,
    )
}

/// Contacts closer than this many f32 units of the largest coordinate count as exact (`snap`).
const SNAP_ULPS: f64 = 2.0;
/// Between different shells, contacts closer than this many mm count as exact too. The result's
/// vertices lie on the grid, so parts closer than half a step touch there: a part resting on a face
/// its CAD export tilted by 0.0001 mm joins the body.
const SNAP_APART: f64 = GRID / 2.0;

/// Makes near contacts exact before the arrangement. CAD exports round each part's coordinates on
/// their own, so a corner of one part often lies within an f32 unit of another part's edge or face
/// instead of on it, or a hair inside it; the exact arrangement then cuts tiny pieces round it,
/// closer together than f32 can hold, and rounding breaks them. Here vertices within `SNAP_ULPS`
/// units of each other become one (the lowest index stays); an edge with a vertex that close to its
/// inside is split there, in both of its triangles; a vertex that close to the inside of a triangle
/// becomes a corner of it (the triangle is triangulated anew round it, in its own parameter space).
/// Between a vertex and another shell (shells as joined by shared vertices) "close" is
/// `SNAP_APART` mm when that is more; within a shell it stays `SNAP_ULPS` units, so no thin wall
/// or short edge of a part is glued shut. The contact is then exact: the parts touch at that
/// vertex. Only welded vertices move, by less than the distance that let them weld; a triangle
/// whose own corner lands on its opposite edge is flat and goes (its neighbour across that edge
/// takes the corner). The mesh and its origin map; None when nothing is that close.
///
/// Every edge must stay balanced (as many sides one way as the other) as the arrangement gets it,
/// or the winding numbers the classification counts stop adding up and whole patches come out
/// wrong. Where stacked thin caps make the snap unbalance an edge the input had balanced, the
/// vertices of that edge stay still and the snap runs again; when that does not settle, nothing
/// snaps.
fn snap_contacts(mesh: &Mesh, control: &mut Control) -> RepairResult<Option<(Mesh, Vec<u32>)>> {
    // Balance as the arrangement gets it, after arrangement_input: copies of a vertex are one, and
    // flat triangles are gone.
    let arranged = |m: &Mesh| unbalanced(arrangement_input(m).0.indices.as_chunks::<3>().0);
    // arrangement_input welds each vertex into the first with its coordinates.
    let mut welder = Welder::new(mesh.indices.len());
    let mut first: Vec<u32> = Vec::new();
    let same: Vec<u32> = (0..mesh.vertex_count() as u32)
        .map(|v| {
            let g = welder.vertex(mesh.vertex(v)) as usize;
            if g == first.len() {
                first.push(v);
            }
            first[g]
        })
        .collect();
    // A vertex on an unbalanced edge in the result that was on none in the input marks the snap's
    // doing. Cracks the input has (left by flat triangles) may close, or stay as they are.
    let mut cracked = vec![false; mesh.vertex_count()];
    for v in arranged(mesh) {
        cracked[v as usize] = true;
    }
    let mut frozen = vec![false; mesh.vertex_count()];
    for attempt in 0..SNAP_TRIES {
        let snapped = snap_once(mesh, &frozen, control.cancel_flag());
        control.check()?;
        control.progress((attempt + 1) as f64 / SNAP_TRIES as f64 * 0.05);
        let Some((out, origin)) = snapped else {
            return Ok(None);
        };
        let mut bad = vec![false; mesh.vertex_count()];
        let mut any = false;
        for v in arranged(&out) {
            if !cracked[v as usize] {
                bad[v as usize] = true;
                any = true;
            }
        }
        if !any {
            return Ok(Some((out, origin)));
        }
        // The damage moves next door when only its own vertices stay still: the triangles round
        // them stay still too.
        for t in mesh.indices.as_chunks::<3>().0 {
            if t.iter().any(|&v| bad[same[v as usize] as usize]) {
                for &v in t {
                    frozen[v as usize] = true;
                }
            }
        }
        for (v, &g) in same.iter().enumerate() {
            frozen[v] |= bad[g as usize];
        }
    }
    Ok(None)
}

/// Runs of the snap at most, each with more vertices kept still.
const SNAP_TRIES: usize = 4;

/// The vertices of edges with more sides one way than the other (sorted).
fn unbalanced(tris: &[[u32; 3]]) -> Vec<u32> {
    let mut sides: Vec<(u32, u32, i32)> = tris
        .par_iter()
        .flat_map_iter(|t| {
            (0..3).filter_map(move |k| {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                match a.cmp(&b) {
                    Ordering::Less => Some((a, b, 1)),
                    Ordering::Greater => Some((b, a, -1)),
                    Ordering::Equal => None,
                }
            })
        })
        .collect();
    sides.par_sort_unstable_by_key(|s| (s.0, s.1));
    let mut out: Vec<u32> = sides
        .chunk_by(|x, y| (x.0, x.1) == (y.0, y.1))
        .filter(|run| run.iter().map(|s| s.2).sum::<i32>() != 0)
        .flat_map(|run| [run[0].0, run[0].1])
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// One run of `snap_contacts`; `frozen` vertices neither weld, nor split an edge, nor get one
/// split at them. None also when `cancel` is set (the caller checks it).
fn snap_once(mesh: &Mesh, frozen: &[bool], cancel: &AtomicBool) -> Option<(Mesh, Vec<u32>)> {
    let stop = || cancel.load(Atomic::Relaxed);
    let still = |v: u32| frozen[v as usize];
    let largest = mesh
        .positions
        .iter()
        .fold(0.0f64, |m, &x| m.max((x as f64).abs()));
    let near = SNAP_ULPS * ulp_f32(largest);
    let apart = near.max(SNAP_APART);
    let shell = crate::mesh::vertex_roots(mesh);
    // How close v must be to w (or to a triangle round w) to snap.
    let limit = |v: u32, w: u32| {
        if shell[v as usize] == shell[w as usize] {
            near
        } else {
            apart
        }
    };
    // The search reaches as far as any snap does.
    let delta = apart;
    let n = mesh.vertex_count();
    // Vertices within delta, joined: sorted by cell (of size delta), a pair lies in neighbouring
    // cells.
    let cell = |p: P3| p.map(|x| (x / delta).floor() as i64);
    let mut by_cell: Vec<([i64; 3], u32)> = (0..n as u32)
        .into_par_iter()
        .map(|v| (cell(mesh.point(v)), v))
        .collect();
    by_cell.par_sort_unstable();
    let close: Vec<(u32, u32)> = by_cell
        .par_iter()
        .flat_map_iter(|&(c, v)| {
            let p = mesh.point(v);
            let mut found = Vec::new();
            if stop() {
                return found;
            }
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let key = [c[0] + dx, c[1] + dy, c[2] + dz];
                        let first = by_cell.partition_point(|x| x.0 < key);
                        for &(k, w) in &by_cell[first..] {
                            if k != key {
                                break;
                            }
                            if w > v
                                && !still(v)
                                && !still(w)
                                && exact::length(exact::sub(mesh.point(w), p)) <= limit(v, w)
                            {
                                found.push((v, w));
                            }
                        }
                    }
                }
            }
            found
        })
        .collect();
    drop(by_cell);
    if stop() {
        return None;
    }
    let welded = !close.is_empty();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    for (v, w) in close {
        join(&mut parent, v, w);
    }
    let to: Vec<u32> = (0..n as u32).map(|v| root(&mut parent, v)).collect();
    let tris: Vec<[u32; 3]> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| t.map(|v| to[v as usize]))
        .collect();
    let point = |v: u32| mesh.point(v);
    let flat =
        |t: &[u32; 3]| !distinct(*t) || exact::collinear(point(t[0]), point(t[1]), point(t[2]));
    // Near contacts of vertices with edges and with the inside of triangles.
    let boxes: Vec<bvh::Aabb> = tris
        .iter()
        .map(|t| {
            let mut b = bvh::EMPTY;
            for &v in t {
                bvh::include(&mut b, point(v));
            }
            grown(b, delta)
        })
        .collect();
    let tree = Bvh::new(&boxes);
    let used: Vec<u32> = {
        let mut u: Vec<u32> = tris
            .iter()
            .filter(|t| !flat(t))
            .flatten()
            .copied()
            .collect();
        u.sort_unstable();
        u.dedup();
        u
    };
    // (edge lo, edge hi, vertex, parameter from lo) and (triangle, vertex).
    type Near = (Vec<(u32, u32, u32, f64)>, Vec<(u32, u32)>);
    let found: Vec<Near> = used
        .par_iter()
        .filter(|&&v| !still(v) && !stop())
        .map(|&v| {
            let p = point(v);
            let (mut on_edges, mut inside) = (Vec::new(), Vec::new());
            tree.query(&[p, p], |t| {
                let tri = tris[t as usize];
                if tri.contains(&v) || flat(&tri) || tri.iter().any(|&w| still(w)) {
                    return;
                }
                let q = tri.map(point);
                if (0..3).any(|k| exact::length(exact::sub(q[k], p)) <= limit(v, tri[k])) {
                    return;
                }
                let reach = limit(v, tri[0]);
                let mut near_edge = false;
                for k in 0..3 {
                    let (a, b) = (tri[k], tri[(k + 1) % 3]);
                    let (pa, pb) = (q[k], q[(k + 1) % 3]);
                    let d = exact::sub(pb, pa);
                    let s = exact::dot(exact::sub(p, pa), d) / exact::dot(d, d);
                    let off = exact::length(exact::sub(p, exact::add(pa, exact::scale(d, s))));
                    if s > 0.0 && s < 1.0 && off <= reach {
                        near_edge = true;
                        let (lo, hi, s) = if a < b { (a, b, s) } else { (b, a, 1.0 - s) };
                        on_edges.push((lo, hi, v, s));
                    }
                }
                if near_edge {
                    return;
                }
                let nrm = exact::normal(q[0], q[1], q[2]);
                let h = exact::dot(exact::sub(p, q[0]), nrm) / exact::length(nrm);
                let within = (0..3).all(|k| {
                    let e = exact::cross(exact::sub(q[(k + 1) % 3], q[k]), exact::sub(p, q[k]));
                    exact::dot(e, nrm) > 0.0
                });
                if within && h.abs() <= reach {
                    inside.push((t, v));
                }
            });
            (on_edges, inside)
        })
        .collect();
    if stop() {
        return None;
    }
    let mut on_edges: Vec<(u32, u32, u32, f64)> = Vec::new();
    let mut inside: Vec<(u32, u32)> = Vec::new();
    for (e, i) in found {
        on_edges.extend(e);
        inside.extend(i);
    }
    if !welded && on_edges.is_empty() && inside.is_empty() {
        return None;
    }
    on_edges.sort_by(|x, y| {
        (x.0, x.1)
            .cmp(&(y.0, y.1))
            .then(x.3.total_cmp(&y.3))
            .then(x.2.cmp(&y.2))
    });
    on_edges.dedup_by(|x, y| (x.0, x.1, x.2) == (y.0, y.1, y.2));
    inside.sort_unstable();
    inside.dedup();
    spread_onto_collapsing(&tris, &point, &flat, &still, &mut on_edges);
    let (indices, origin) = subdivide(&tris, &point, &flat, &on_edges, &inside);
    Some((
        Mesh {
            positions: mesh.positions.clone(),
            indices,
        },
        origin,
    ))
}

/// A triangle whose own corner v lies on its opposite edge a–b is flat and goes (`subdivide`): it
/// collapses onto that edge, its neighbour across it taking v. So every point on any of its three
/// edges must end up on all the edges it collapses onto: on a–b, and on a–v or v–b by where it lies
/// along a–b against v. Then its neighbours split their edges alike and the surface stays closed.
/// That can make more such triangles; a few rounds settle it. `on_edges` (lo, hi, vertex,
/// parameter from lo) gets the points that adds, sorted by edge and parameter.
fn spread_onto_collapsing(
    tris: &[[u32; 3]],
    point: &(impl Fn(u32) -> P3 + Sync),
    flat: &(impl Fn(&[u32; 3]) -> bool + Sync),
    still: &(impl Fn(u32) -> bool + Sync),
    on_edges: &mut Vec<(u32, u32, u32, f64)>,
) {
    let entry = |lo: u32, hi: u32, w: u32| {
        let dd = exact::sub(point(hi), point(lo));
        let s = exact::dot(exact::sub(point(w), point(lo)), dd) / exact::dot(dd, dd);
        (lo, hi, w, s.clamp(1e-9, 1.0 - 1e-9))
    };
    for _ in 0..4 {
        let more: Vec<(u32, u32, u32, f64)> = tris
            .par_iter()
            .filter(|t| !flat(t))
            .flat_map_iter(|&tri| {
                let mut found = Vec::new();
                for k in 0..3 {
                    let (a, b, v) = (tri[k], tri[(k + 1) % 3], tri[(k + 2) % 3]);
                    if !points_on(on_edges, a, b).iter().any(|x| x.2 == v) {
                        continue;
                    }
                    let (pa, pb) = (point(a), point(b));
                    let d = exact::sub(pb, pa);
                    let along = |x: u32| exact::dot(exact::sub(point(x), pa), d);
                    let mut points: Vec<u32> = [(a, b), (a, v), (v, b)]
                        .iter()
                        .flat_map(|&(x, y)| points_on(on_edges, x, y).iter().map(|e| e.2))
                        .filter(|w| !tri.contains(w) && !still(*w))
                        .collect();
                    points.sort_unstable();
                    points.dedup();
                    for w in points {
                        found.push(entry(a.min(b), a.max(b), w));
                        let (e0, e1) = if along(w) < along(v) { (a, v) } else { (v, b) };
                        found.push(entry(e0.min(e1), e0.max(e1), w));
                    }
                }
                found
            })
            .filter(|x| points_on(on_edges, x.0, x.1).iter().all(|y| y.2 != x.2))
            .collect();
        if more.is_empty() {
            break;
        }
        on_edges.extend(more);
        on_edges.sort_by(|x, y| {
            (x.0, x.1)
                .cmp(&(y.0, y.1))
                .then(x.3.total_cmp(&y.3))
                .then(x.2.cmp(&y.2))
        });
        on_edges.dedup_by(|x, y| (x.0, x.1, x.2) == (y.0, y.1, y.2));
    }
}

/// The triangles with the points `on_edges` (lo, hi, vertex, parameter from lo; sorted by edge and
/// parameter) on their edges and the points `inside` (triangle, vertex; sorted) inside them, each
/// triangulated anew in its own parameter space. Flat triangles go, and so does a triangle with one
/// of its own corners on one of its edges (it collapses onto that edge). The indices and, per
/// triangle, the triangle it came from.
fn subdivide(
    tris: &[[u32; 3]],
    point: &impl Fn(u32) -> P3,
    flat: &impl Fn(&[u32; 3]) -> bool,
    on_edges: &[(u32, u32, u32, f64)],
    inside: &[(u32, u32)],
) -> (Vec<u32>, Vec<u32>) {
    let edge_points = |a: u32, b: u32| points_on(on_edges, a, b);
    let mut indices = Vec::with_capacity(tris.len() * 3);
    let mut origin = Vec::with_capacity(tris.len());
    for (t, &tri) in tris.iter().enumerate() {
        if flat(&tri) {
            continue;
        }
        let interior: Vec<u32> = {
            let s = inside.partition_point(|x| x.0 < t as u32);
            let e = inside.partition_point(|x| x.0 <= t as u32);
            inside[s..e].iter().map(|x| x.1).collect()
        };
        let mut boundary: Vec<(u32, P2)> = Vec::new();
        let corners: [P2; 3] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let mut lands_on_itself = false;
        for k in 0..3 {
            boundary.push((tri[k], corners[k]));
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            let list = edge_points(a, b);
            let mut add = |&(_, _, v, s): &(u32, u32, u32, f64)| {
                if tri.contains(&v) {
                    lands_on_itself = true;
                    return;
                }
                let s = if a < b { s } else { 1.0 - s };
                let uv = match k {
                    0 => [s, 0.0],
                    1 => [1.0 - s, s],
                    _ => [0.0, 1.0 - s],
                };
                boundary.push((v, uv));
            };
            if a < b {
                list.iter().for_each(&mut add);
            } else {
                list.iter().rev().for_each(&mut add);
            }
        }
        if lands_on_itself {
            continue;
        }
        if boundary.len() == 3 && interior.is_empty() {
            indices.extend_from_slice(&tri);
            origin.push(t as u32);
            continue;
        }
        // Interior points in the triangle's parameter space (least squares), kept inside.
        let [pa, pb, pc] = tri.map(point);
        let (e1, e2) = (exact::sub(pb, pa), exact::sub(pc, pa));
        let (g11, g12, g22) = (exact::dot(e1, e1), exact::dot(e1, e2), exact::dot(e2, e2));
        let det = g11 * g22 - g12 * g12;
        let mut points: Vec<(u32, P2)> = boundary;
        let ring = points.len();
        for &v in &interior {
            let r = exact::sub(point(v), pa);
            let (r1, r2) = (exact::dot(r, e1), exact::dot(r, e2));
            let u = (g22 * r1 - g12 * r2) / det;
            let w = (g11 * r2 - g12 * r1) / det;
            let margin = 1e-9;
            let (u, w) = (u.max(margin), w.max(margin));
            let (u, w) = if u + w >= 1.0 - margin {
                let s = (1.0 - margin) / (u + w);
                (u * s, w * s)
            } else {
                (u, w)
            };
            points.push((v, [u, w]));
        }
        let flat_points: Vec<P2> = points.iter().map(|p| p.1).collect();
        match arrangement::triangulate_flat(&flat_points, ring) {
            Some(faces) => {
                for f in faces {
                    let sub = f.map(|i| points[i].0);
                    if distinct(sub) {
                        indices.extend_from_slice(&sub);
                        origin.push(t as u32);
                    }
                }
            }
            None => {
                indices.extend_from_slice(&tri);
                origin.push(t as u32);
            }
        }
    }
    (indices, origin)
}

/// The points snapped onto edge a–b: the entries (lo, hi, vertex, parameter from lo) of `list`
/// (sorted by edge) for that edge.
fn points_on(list: &[(u32, u32, u32, f64)], a: u32, b: u32) -> &[(u32, u32, u32, f64)] {
    let (lo, hi) = (a.min(b), a.max(b));
    let s = list.partition_point(|x| (x.0, x.1) < (lo, hi));
    let e = list.partition_point(|x| (x.0, x.1) <= (lo, hi));
    &list[s..e]
}

/// The sorted vertex triple of a triangle and whether the triangle is an even permutation of it
/// (faces the same way as the sorted order).
fn sorted(t: [u32; 3]) -> ([u32; 3], bool) {
    let r = if t[0] < t[1] && t[0] < t[2] {
        t
    } else if t[1] < t[2] {
        [t[1], t[2], t[0]]
    } else {
        [t[2], t[0], t[1]]
    };
    if r[1] < r[2] {
        (r, true)
    } else {
        ([r[0], r[2], r[1]], false)
    }
}

/// Triangles over the same three corners (by coordinates) that face opposite ways cancel in pairs:
/// together they enclose nothing (two shells glued face to face, a double-sided sheet). The
/// earliest copies stay. The mesh and its origin map, or None when nothing cancelled.
fn cancel_opposite(mesh: &Mesh) -> Option<(Mesh, Vec<u32>)> {
    let tris = mesh.indices.as_chunks::<3>().0;
    // Corners are the same when their coordinates are, as in doubles::remove_doubles.
    let mut welder = Welder::new(mesh.indices.len());
    let at: Vec<u32> = (0..mesh.vertex_count() as u32)
        .map(|v| welder.vertex(mesh.vertex(v)))
        .collect();
    let mut keys: Vec<([u32; 3], bool, u32)> = tris
        .iter()
        .enumerate()
        .map(|(t, &tri)| {
            let (key, even) = sorted(tri.map(|v| at[v as usize]));
            (key, even, t as u32)
        })
        .collect();
    keys.sort_unstable();
    let mut keep = vec![true; tris.len()];
    let mut any = false;
    for group in keys.chunk_by(|a, b| a.0 == b.0) {
        // Sorted: the odd copies first, then the even ones, each in triangle order.
        let odd = group.partition_point(|k| !k.1);
        let pairs = odd.min(group.len() - odd);
        for k in group[odd - pairs..odd]
            .iter()
            .chain(&group[group.len() - pairs..])
        {
            keep[k.2 as usize] = false;
            any = true;
        }
    }
    if !any {
        return None;
    }
    let mut indices = Vec::with_capacity(mesh.indices.len());
    let mut origin = Vec::with_capacity(tris.len());
    for (t, tri) in tris.iter().enumerate() {
        if keep[t] {
            indices.extend_from_slice(tri);
            origin.push(t as u32);
        }
    }
    Some((
        Mesh {
            positions: mesh.positions.clone(),
            indices,
        },
        origin,
    ))
}

/// Removes triangles whose three different corners lie exactly on one line, without opening the
/// mesh: such a triangle (a, m, c), m between a and c, and the triangle (a, c, d) across its long
/// side become (a, m, d) and (m, c, d), an edge flip that keeps every edge two-sided. Rounding new
/// points to f32 can leave such slivers. When the triangle across is flat too, the flip waits for
/// it, unless d also lies between a and c: then the two flat triangles span the same long side, and
/// flipping them shortens both. A flat triangle whose long side has no single opposite partner
/// stays. The mesh and its origin map, or None when nothing changed. Remove degenerate faces flips
/// what its own fixes leave the same way.
pub(crate) fn zip_collinear(mesh: &Mesh) -> Option<(Mesh, Vec<u32>)> {
    let mut tris: Vec<[u32; 3]> = mesh.indices.as_chunks::<3>().0.to_vec();
    let point = |v: u32| mesh.point(v);
    let mut flat = vec![false; tris.len()];
    let mut pending: Vec<u32> = Vec::new();
    for (t, &tri) in tris.iter().enumerate() {
        let [a, b, c] = tri;
        if distinct(tri) && exact::collinear(point(a), point(b), point(c)) {
            flat[t] = true;
            pending.push(t as u32);
        }
    }
    if pending.is_empty() {
        return None;
    }
    let mut origin: Vec<u32> = (0..tris.len() as u32).collect();
    let mut flipped_any = false;
    let mut marked = vec![false; mesh.vertex_count()];
    let mut touched = vec![false; tris.len()];
    for _pass in 0..64 {
        // Each pending triangle as (a, m, c), m the middle corner.
        for &t in &pending {
            tris[t as usize] = middle_second(tris[t as usize], &point);
        }
        // The sides on each pending triangle's long edge, and every edge at its corners.
        let mut wanted: Vec<(u32, u32)> = pending
            .iter()
            .map(|&t| {
                let [a, _, c] = tris[t as usize];
                (a.min(c), a.max(c))
            })
            .collect();
        wanted.sort_unstable();
        wanted.dedup();
        for &t in &pending {
            for v in tris[t as usize] {
                marked[v as usize] = true;
            }
        }
        let mut sides: Vec<((u32, u32), u32)> = Vec::new();
        let mut links: Vec<(u32, u32)> = Vec::new();
        for (t, tri) in tris.iter().enumerate() {
            for k in 0..3 {
                let (x, y) = (tri[k], tri[(k + 1) % 3]);
                if !marked[x as usize] && !marked[y as usize] {
                    continue;
                }
                let e = (x.min(y), x.max(y));
                links.push(e);
                if wanted.binary_search(&e).is_ok() {
                    sides.push((e, (t * 3 + k) as u32));
                }
            }
        }
        sides.sort_unstable();
        links.sort_unstable();
        for m in marked.iter_mut() {
            *m = false;
        }
        let mut made: Vec<(u32, u32)> = Vec::new();
        let mut progress = false;
        let mut left = Vec::new();
        for &t in &pending {
            let [a, m, c] = tris[t as usize];
            let e = (a.min(c), a.max(c));
            let start = sides.partition_point(|s| s.0 < e);
            let on = &sides[start..sides.partition_point(|s| s.0 <= e)];
            let partner = match on {
                [x, y] if x.1 / 3 == t => y.1,
                [x, y] if y.1 / 3 == t => x.1,
                _ => {
                    left.push(t);
                    continue;
                }
            };
            let (n, k) = ((partner / 3) as usize, (partner % 3) as usize);
            let other = tris[n];
            let d = other[(k + 2) % 3];
            let new_edge = (m.min(d), m.max(d));
            // The partner must run a → c (the opposite way), be untouched this pass, and be fit to
            // split; the flip must not double an edge.
            let fit = !flat[n] || between(point(a), point(d), point(c));
            if touched[t as usize]
                || touched[n]
                || other[k] != a
                || other[(k + 1) % 3] != c
                || !fit
                || links.binary_search(&new_edge).is_ok()
                || made.contains(&new_edge)
            {
                left.push(t);
                continue;
            }
            tris[n] = [a, m, d];
            tris[t as usize] = [m, c, d];
            made.push(new_edge);
            if flat[n] {
                // Both halves lie on the line still; both stay pending (n already is).
                left.push(t);
            } else {
                origin[t as usize] = origin[n];
                flat[t as usize] = false;
            }
            touched[t as usize] = true;
            touched[n] = true;
            progress = true;
            flipped_any = true;
        }
        for t in touched.iter_mut() {
            *t = false;
        }
        pending = left;
        if !progress || pending.is_empty() {
            break;
        }
    }
    if !flipped_any {
        return None;
    }
    Some((
        Mesh {
            positions: mesh.positions.clone(),
            indices: tris.into_flattened(),
        },
        origin,
    ))
}

/// The axis along which three collinear points spread most.
fn main_axis(p: [P3; 3]) -> usize {
    let spread = |k: usize| {
        let (lo, hi) = p
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| {
                (lo.min(q[k]), hi.max(q[k]))
            });
        hi - lo
    };
    (0..3)
        .max_by(|&x, &y| spread(x).total_cmp(&spread(y)))
        .unwrap_or(0)
}

/// True when `m` lies strictly between `a` and `c` (the three points being on one line).
fn between(a: P3, m: P3, c: P3) -> bool {
    let k = main_axis([a, m, c]);
    (a[k] < m[k] && m[k] < c[k]) || (c[k] < m[k] && m[k] < a[k])
}

/// A flat triangle rotated (keeping its orientation) so its middle corner comes second.
fn middle_second(t: [u32; 3], point: &impl Fn(u32) -> P3) -> [u32; 3] {
    let p = t.map(point);
    let middle = (0..3)
        .find(|&k| between(p[(k + 2) % 3], p[k], p[(k + 1) % 3]))
        .unwrap_or(1);
    [t[(middle + 2) % 3], t[middle], t[(middle + 1) % 3]]
}

// ---------------------------------------------------------------------------------------------
// 1. Shells inside shells

/// Six times the signed volume of triangle (a, b, c) with the apex at `o`.
fn volume6(o: P3, [a, b, c]: [P3; 3]) -> f64 {
    exact::dot(
        exact::sub(a, o),
        exact::cross(exact::sub(b, o), exact::sub(c, o)),
    )
}

/// Which triangles stay when closed shells inside other closed shells, and closed shells with no
/// volume, go; None when nothing goes. Shells are triangles joined through shared vertices; closed
/// means every edge has two opposite sides.
fn inner_or_flat_shells(mesh: &Mesh, control: &mut Control) -> RepairResult<Option<Vec<bool>>> {
    let edges = Edges::new(mesh);
    let (shell_of, count) = topology::components(mesh, &edges, Link::Vertex);
    let mut closed = vec![true; count];
    for e in 0..edges.len() as u32 {
        if !topology::is_consistent(mesh, &edges, e) {
            for &side in edges.sides(e) {
                closed[shell_of[side as usize / 3] as usize] = false;
            }
        }
    }
    drop(edges);
    let mut members: Vec<Vec<u32>> = vec![Vec::new(); count];
    for (t, &s) in shell_of.iter().enumerate() {
        members[s as usize].push(t as u32);
    }
    let closed_ids: Vec<usize> = (0..count).filter(|&s| closed[s]).collect();
    if closed_ids.is_empty() {
        return Ok(None);
    }
    let lists: Vec<Vec<u32>> = closed_ids
        .iter()
        .map(|&s| std::mem::take(&mut members[s]))
        .collect();
    let depths = winding::nesting_depths(mesh, &lists, control)?;
    let flat = FLAT_SHELL * topology::bbox_diagonal(mesh).powi(3);
    let tris = mesh.indices.as_chunks::<3>().0;
    let mut gone = vec![false; count];
    for (k, &s) in closed_ids.iter().enumerate() {
        let o = mesh.point(tris[lists[k][0] as usize][0]);
        let volume: f64 = lists[k]
            .iter()
            .map(|&t| volume6(o, tris[t as usize].map(|v| mesh.point(v))))
            .sum::<f64>()
            / 6.0;
        gone[s] = depths[k] > 0 || volume.abs() <= flat;
    }
    if !gone.contains(&true) {
        return Ok(None);
    }
    Ok(Some(shell_of.iter().map(|&s| !gone[s as usize]).collect()))
}

// ---------------------------------------------------------------------------------------------
// 2. Classification

/// A mesh of bare indices over `count` points, for the topology helpers (positions unused).
fn index_mesh(count: usize, triangles: &[[u32; 3]]) -> Mesh {
    Mesh {
        positions: vec![0.0; count * 3],
        indices: triangles.as_flattened().to_vec(),
    }
}

fn oriented(t: [u32; 3], side: u8) -> [u32; 3] {
    if side == TURN { [t[0], t[2], t[1]] } else { t }
}

/// Union-find root with path halving.
fn root(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        let up = parent[parent[x as usize] as usize];
        parent[x as usize] = up;
        x = up;
    }
    x
}

fn join(parent: &mut [u32], a: u32, b: u32) {
    let (ra, rb) = (root(parent, a), root(parent, b));
    if ra != rb {
        parent[ra.max(rb) as usize] = ra.min(rb);
    }
}

/// The far end of a ray from `p` along `d`: beyond the sphere around `reach`.
fn far(p: P3, d: P3, reach: &Aabb) -> P3 {
    let center = [0, 1, 2].map(|k| (reach[0][k] + reach[1][k]) * 0.5);
    let radius = exact::length(exact::sub(reach[1], reach[0])) * 0.5;
    let distance = 2.0 * (exact::length(exact::sub(p, center)) + radius) + 1.0;
    exact::add(p, exact::scale(d, distance / exact::length(d)))
}

/// The rest of a surface round a region that is wrapped again on its own (`rewrap_spots`): its
/// faces outside the region count for the winding numbers too.
struct Outside<'a> {
    mesh: &'a Mesh,
    tree: &'a Bvh,
    /// Per face of `mesh`: in the region (so it does not count here).
    skip: &'a [bool],
}

/// Rays cast against the arrangement's input triangles (and the faces `outside`, if any), from
/// points that may be implicit.
struct Rays<'a> {
    arr: &'a Arrangement,
    tree: Bvh,
    reach: Aabb,
    outside: Option<Outside<'a>>,
}

impl<'a> Rays<'a> {
    fn new(arr: &'a Arrangement, outside: Option<Outside<'a>>) -> Rays<'a> {
        let tree = Bvh::new(&bvh::boxes_of(&arr.input.points, &arr.input.tris));
        let mut reach = tree.bounds().unwrap_or([[0.0; 3]; 2]);
        if let Some(b) = outside.as_ref().and_then(|o| o.tree.bounds()) {
            bvh::grow(&mut reach, &b);
        }
        Rays {
            arr,
            tree,
            reach,
            outside,
        }
    }

    /// The winding number just in front of `p`, a point on a face whose (oriented) normal is
    /// `normal`, from a ray leaving towards the front: the sum over the triangles it crosses of +1
    /// where it leaves through a triangle's front, −1 where it enters. With `on_surface`, triangles
    /// whose plane holds p do not count: the ray only touches their plane at its start. None when
    /// every direction tried met an edge or lay in a plane.
    fn in_front(&self, p: &Pt, normal: P3, on_surface: bool) -> Option<i32> {
        let n = exact::scale(normal, 1.0 / exact::length(normal));
        let mut hits = Vec::new();
        let mut others = Vec::new();
        'directions: for d in DIRECTIONS {
            let dir = exact::add(n, exact::scale(d, 0.25 / exact::length(d)));
            let q = far(p.p, dir, &self.reach);
            hits.clear();
            self.tree.segment(p.p, q, |t| hits.push(t));
            let mut w = 0;
            for &t in &hits {
                match self.crossing(self.arr.input.corners(t), p, q, on_surface) {
                    Some(s) => w += s,
                    None => continue 'directions,
                }
            }
            if let Some(o) = &self.outside {
                others.clear();
                o.tree.segment(p.p, q, |t| {
                    if !o.skip[t as usize] {
                        others.push(t);
                    }
                });
                for &t in &others {
                    match self.crossing(exact::corners(o.mesh, t), p, q, on_surface) {
                        Some(s) => w += s,
                        None => continue 'directions,
                    }
                }
            }
            return Some(w);
        }
        None
    }

    /// How the ray from p to q meets triangle `tri`: +1 from its back to its front, −1 the other
    /// way, 0 not at all; None when it meets an edge or a corner or lies in the plane.
    fn crossing(&self, tri: [P3; 3], p: &Pt, q: P3, on_surface: bool) -> Option<i32> {
        let input = &self.arr.input;
        let sp = arrangement::orient3d_plane(input, tri, p);
        let sq = exact::orient3d(tri[0], tri[1], tri[2], q);
        if sp == 0 {
            return (on_surface && sq != 0).then_some(0);
        }
        if sq == 0 {
            return None;
        }
        if sp == sq {
            return Some(0);
        }
        // orient3d(p, q, a, b) for each edge (a, b), as −orient3d(q, a, b, p).
        let e = [(0, 1), (1, 2), (2, 0)]
            .map(|(i, j)| -arrangement::orient3d_plane(input, [q, tri[i], tri[j]], p));
        if e.contains(&1) && e.contains(&-1) {
            Some(0)
        } else if e.contains(&0) {
            None
        } else {
            Some(if sp < 0 { 1 } else { -1 })
        }
    }
}

/// Per face of the arrangement: DROP, KEEP or TURN. A patch (faces joined across edges with
/// exactly two consistently oriented faces) separates the same two winding numbers everywhere: w
/// just in front of it, w + c behind it, c the face's coverage. It belongs to the outer surface
/// when exactly one of the two is zero, and must face the zero side. One probe decides each patch:
/// a ray from a point exactly on one of its faces, the largest that can hold such a point.
fn classify(
    arr: &Arrangement,
    outside: Option<Outside>,
    control: &mut Control,
) -> RepairResult<Vec<u8>> {
    let faces = &arr.faces;
    let index = index_mesh(arr.point_count(), faces);
    let edges = Edges::new(&index);
    let mut parent: Vec<u32> = (0..faces.len() as u32).collect();
    for e in 0..edges.len() as u32 {
        if let [s, r] = *edges.sides(e)
            && topology::side_ends(&index, s).0 != topology::side_ends(&index, r).0
        {
            join(&mut parent, s / 3, r / 3);
        }
    }
    let mut patch_of = vec![0u32; faces.len()];
    let mut patches = 0usize;
    for f in 0..faces.len() as u32 {
        let r = root(&mut parent, f);
        patch_of[f as usize] = if r == f {
            patches += 1;
            (patches - 1) as u32
        } else {
            patch_of[r as usize]
        };
    }
    control.check()?;
    let area: Vec<f64> = faces
        .par_iter()
        .map(|t| {
            let p = t.map(|v| arr.pt(v).p);
            exact::length(exact::normal(p[0], p[1], p[2]))
        })
        .collect();
    let mut order: Vec<u32> = (0..faces.len() as u32).collect();
    order.par_sort_by(|&x, &y| {
        patch_of[x as usize]
            .cmp(&patch_of[y as usize])
            .then(area[y as usize].total_cmp(&area[x as usize]))
            .then(x.cmp(&y))
    });
    let mut start = vec![0usize; patches + 1];
    for &p in &patch_of {
        start[p as usize + 1] += 1;
    }
    for p in 0..patches {
        start[p + 1] += start[p];
    }
    let rays = Rays::new(arr, outside);
    let cancel = control.cancel_flag();
    let normal = |f: u32| {
        let [a, b, c] = arr.input.corners(arr.source[f as usize]);
        exact::normal(a, b, c)
    };
    let mut winding: Vec<Option<i32>> = (0..patches)
        .into_par_iter()
        .map(|p| {
            if cancel.load(Atomic::Relaxed) {
                return None;
            }
            let faces = &order[start[p]..start[p + 1]];
            faces.iter().take(PROBES).find_map(|&f| {
                let probe = arr.probe(f as usize)?;
                rays.in_front(&probe, normal(f), true)
            })
        })
        .collect();
    control.check()?;
    // Slivers between nearly coincident layers are too thin for an exact point: their patches take
    // the winding number from a neighbour's, exactly, round an edge they share.
    from_neighbours(arr, &index, &edges, &patch_of, &mut winding);
    drop((edges, index));
    let missing: Vec<usize> = (0..patches).filter(|&p| winding[p].is_none()).collect();
    let guessed: Vec<Option<i32>> = missing
        .par_iter()
        .map(|&p| {
            // Every face of the patch is too thin for an exact point on it: a point just in front
            // of the largest one instead.
            let faces = &order[start[p]..start[p + 1]];
            let f = faces[0];
            let tri = arr.faces[f as usize].map(|v| arr.pt(v).p);
            let centroid = [0, 1, 2].map(|k| (tri[0][k] + tri[1][k] + tri[2][k]) / 3.0);
            let n = normal(f);
            let size = exact::length(exact::sub(tri[1], tri[0])).max(f64::MIN_POSITIVE);
            let p = exact::add(centroid, exact::scale(n, 1e-9 * size / exact::length(n)));
            let probe = Pt {
                g: arrangement::Geom::V(0),
                p,
                e: 0.0,
            };
            rays.in_front(&probe, n, false)
        })
        .collect();
    for (&p, w) in missing.iter().zip(guessed) {
        winding[p] = w;
    }
    control.check()?;
    Ok((0..faces.len())
        .map(|f| {
            let Some(front) = winding[patch_of[f] as usize] else {
                return DROP;
            };
            let back = front + arr.coverage[f] as i32;
            if (front == 0) == (back == 0) {
                DROP
            } else if front == 0 {
                KEEP
            } else {
                TURN
            }
        })
        .collect())
}

/// Fills in the winding numbers of patches that have none from a neighbouring patch's: round an
/// edge, in the exact counter-clockwise order of its faces (`round_edge`), the winding number
/// changes by each face's coverage, up where its back lies counter-clockwise and down where it
/// lies clockwise. An edge counts only when every face on it with a known winding number agrees.
fn from_neighbours(
    arr: &Arrangement,
    index: &Mesh,
    edges: &Edges,
    patch_of: &[u32],
    winding: &mut [Option<i32>],
) {
    let patch = |side: u32| patch_of[(side / 3) as usize] as usize;
    let mut todo: Vec<u32> = (0..edges.len() as u32)
        .filter(|&e| {
            let sides = edges.sides(e);
            sides.len() > 2 && sides.iter().any(|&s| winding[patch(s)].is_none())
        })
        .collect();
    if todo.is_empty() {
        return;
    }
    let turned = vec![false; arr.faces.len()];
    loop {
        let before = todo.len();
        todo.retain(|&e| {
            let sides = edges.sides(e);
            if !sides.iter().any(|&s| winding[patch(s)].is_some()) {
                return true;
            }
            let (ring, open) = round_edge(arr, index, edges, e, &arr.source, &turned);
            // The winding number in front of each face, up to one constant for the whole ring.
            let mut w = 0i32;
            let fronts: Vec<i32> = ring
                .iter()
                .zip(&open)
                .map(|(&s, &open)| {
                    let c = arr.coverage[(s / 3) as usize] as i32;
                    if open {
                        let front = w;
                        w += c;
                        front
                    } else {
                        w -= c;
                        w
                    }
                })
                .collect();
            let known: Vec<i32> = ring
                .iter()
                .zip(&fronts)
                .filter_map(|(&s, &f)| winding[patch(s)].map(|w| w - f))
                .collect();
            if w != 0 || known.iter().any(|&k| k != known[0]) {
                return false;
            }
            for (&s, &f) in ring.iter().zip(&fronts) {
                winding[patch(s)].get_or_insert(f + known[0]);
            }
            false
        });
        if todo.len() == before || todo.is_empty() {
            break;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 3. The outer surface

/// The kept outer surface before rounding: triangles over vertices at their exact positions (in
/// f64, within an ulp), and what rounding needs to know.
struct Surface {
    tris: Vec<[u32; 3]>,
    points: Vec<P3>,
    /// Per vertex: the arrangement's point it stands for.
    ids: Vec<u32>,
    /// Per triangle: the arrangement's input triangle it came from, and the normal of the plane it
    /// lies in exactly, facing its way.
    source: Vec<u32>,
    normals: Vec<P3>,
}

impl Surface {
    /// Only the triangles `keep`, with the vertices they use (in their order).
    fn keep(self, keep: &[bool]) -> Surface {
        let mut new_id = vec![NONE_U32; self.points.len()];
        let mut points = Vec::new();
        let mut ids = Vec::new();
        let mut tris = Vec::new();
        let mut source = Vec::new();
        let mut normals = Vec::new();
        for (t, tri) in self.tris.iter().enumerate() {
            if !keep[t] {
                continue;
            }
            tris.push(tri.map(|v| {
                if new_id[v as usize] == NONE_U32 {
                    new_id[v as usize] = points.len() as u32;
                    points.push(self.points[v as usize]);
                    ids.push(self.ids[v as usize]);
                }
                new_id[v as usize]
            }));
            source.push(self.source[t]);
            normals.push(self.normals[t]);
        }
        Surface {
            tris,
            points,
            ids,
            source,
            normals,
        }
    }

    /// Without cavities: shells (joined through edges) that are closed and enclose negative
    /// volume, voids inside the union that wrapping fills.
    fn without_cavities(self) -> Surface {
        match without_dust(&self.tris, &|v| self.points[v as usize], self.points.len()) {
            Some(keep) => self.keep(&keep),
            None => self,
        }
    }
}

/// The faces `decision` keeps: where bodies touch along an edge, the faces round it are paired so
/// that each pair bounds one wedge of solid (`round_edge`, `match_round`), or, where that leaves a
/// pinch, so that each pair bounds an empty wedge; then every vertex gets one copy per fan (corners
/// joined through edges with two sides and through the pairs), so the surface is manifold and
/// bodies that touch along an edge are separate shells.
fn surface(arr: &Arrangement, decision: &[u8]) -> Surface {
    let kept: Vec<usize> = (0..arr.faces.len())
        .filter(|&f| decision[f] != DROP)
        .collect();
    let tris: Vec<[u32; 3]> = kept
        .iter()
        .map(|&f| oriented(arr.faces[f], decision[f]))
        .collect();
    let source: Vec<u32> = kept.iter().map(|&f| arr.source[f]).collect();
    let turned: Vec<bool> = kept.iter().map(|&f| decision[f] == TURN).collect();
    let index = index_mesh(arr.point_count(), &tris);
    let edges = Edges::new(&index);
    // Edges with more than two sides, each with its sides in counter-clockwise order round it and
    // which of them open a wedge of solid.
    let rounds: Vec<(u32, Vec<u32>, Vec<bool>)> = (0..edges.len() as u32)
        .filter(|&e| edges.degree(e) > 2)
        .collect::<Vec<u32>>()
        .par_iter()
        .map(|&e| {
            let (ring, open) = round_edge(arr, &index, &edges, e, &source, &turned);
            (e, ring, open)
        })
        .collect();
    // The corners at both ends of a side.
    let ends = |side: u32| [side, side / 3 * 3 + (side % 3 + 1) % 3];
    // The corners at both ends of two sides on one edge, joined end to end.
    let link = |parent: &mut Vec<u32>, s: u32, r: u32| {
        for x in ends(s) {
            for y in ends(r) {
                if index.indices[x as usize] == index.indices[y as usize] {
                    join(parent, x, y);
                }
            }
        }
    };
    let mut across = vec![false; rounds.len()];
    let mut parent: Vec<u32> = Vec::new();
    for _ in 0..4 {
        parent = (0..index.indices.len() as u32).collect();
        for e in 0..edges.len() as u32 {
            if let [s, r] = *edges.sides(e) {
                link(&mut parent, s, r);
            }
        }
        for (k, (_, ring, open)) in rounds.iter().enumerate() {
            for (s, r) in match_round(ring, open, across[k]) {
                link(&mut parent, s, r);
            }
        }
        // A pinch: an edge whose sides still share their end vertices after the split (the
        // surface round an end passes the edge twice). Pairing across the empty wedges there
        // instead splits the round into two.
        let mut pinched = false;
        for (k, (e, _, _)) in rounds.iter().enumerate() {
            let mut keys: Vec<(u32, u32)> = edges
                .sides(*e)
                .iter()
                .map(|&s| {
                    let [x, y] = ends(s).map(|c| root(&mut parent, c));
                    (x.min(y), x.max(y))
                })
                .collect();
            keys.sort_unstable();
            if keys.windows(3).any(|w| w[0] == w[2]) {
                across[k] = !across[k];
                pinched = true;
            }
        }
        if !pinched {
            break;
        }
    }
    let mut copy = vec![NONE_U32; index.indices.len()];
    let mut points: Vec<u32> = Vec::new();
    let mut indices = vec![0u32; index.indices.len()];
    for c in 0..index.indices.len() {
        let r = root(&mut parent, c as u32) as usize;
        if copy[r] == NONE_U32 {
            copy[r] = points.len() as u32;
            points.push(index.indices[c]);
        }
        indices[c] = copy[r];
    }
    let normals = source
        .iter()
        .zip(&turned)
        .map(|(&t, &turn)| {
            let [a, b, c] = arr.input.corners(t);
            let n = exact::normal(a, b, c);
            if turn { exact::scale(n, -1.0) } else { n }
        })
        .collect();
    Surface {
        tris: indices.as_chunks::<3>().0.to_vec(),
        points: points.iter().map(|&p| arr.pt(p).p).collect(),
        ids: points,
        source,
        normals,
    }
}

const NONE_U32: u32 = u32::MAX;

/// The sides on edge e of the kept surface in counter-clockwise order round it, and per side
/// whether it opens a wedge of solid there. Round the edge (lo → hi, direction ℓ) each face lies in
/// the half-plane along u = σ m × ℓ, m its oriented normal (its source triangle's, turned if it was
/// turned) and σ = +1 when its side runs lo → hi; then (u_i × u_j) · ℓ has the sign of
/// σ_i σ_j det(m_i, m_j, ℓ) and u_i · u_j that of σ_i σ_j m_i · m_j, all exact, with ℓ along
/// m_a × m_b for two faces in different planes (its sign from the positions of lo and hi). A face's
/// solid side (its back) lies counter-clockwise of it exactly when σ = −1: such faces open a wedge
/// of solid, the others close one.
fn round_edge(
    arr: &Arrangement,
    index: &Mesh,
    edges: &Edges,
    e: u32,
    source: &[u32],
    turned: &[bool],
) -> (Vec<u32>, Vec<bool>) {
    let [lo, hi] = edges.ends(e);
    let sides = edges.sides(e);
    let m: Vec<E3> = sides
        .iter()
        .map(|&s| {
            let f = (s / 3) as usize;
            let [a, b, c] = arr.input.corners(source[f]);
            let n = arrangement::normal_e(a, b, c);
            if turned[f] {
                n.map(|x| exp::neg(&x))
            } else {
                n
            }
        })
        .collect();
    let sigma: Vec<i32> = sides
        .iter()
        .map(|&s| {
            if topology::side_ends(index, s).0 == lo {
                1
            } else {
                -1
            }
        })
        .collect();
    let mut along: Option<(E3, i32)> = None;
    'find: for i in 0..m.len() {
        for j in i + 1..m.len() {
            let d = arrangement::cross_e(&m[i], &m[j]);
            if d.iter().any(|x| exp::sign(x) != 0) {
                let lambda = arrangement::dir_sign(&arr.input, &d, &arr.pt(lo), &arr.pt(hi));
                along = Some((d, lambda));
                break 'find;
            }
        }
    }
    let cross = |i: usize, j: usize| match &along {
        Some((d, lambda)) => {
            sigma[i]
                * sigma[j]
                * lambda
                * exp::sign(&arrangement::dot_e(&arrangement::cross_e(&m[i], &m[j]), d))
        }
        None => 0,
    };
    let dot =
        |i: usize, j: usize| sigma[i] * sigma[j] * exp::sign(&arrangement::dot_e(&m[i], &m[j]));
    // 0: along u_0, 1: less than a half turn counter-clockwise, 2: a half turn, 3: beyond.
    let half = |i: usize| -> u8 {
        if i == 0 {
            return 0;
        }
        match cross(0, i) {
            1 => 1,
            -1 => 3,
            _ if dot(0, i) > 0 => 0,
            _ => 2,
        }
    };
    let halves: Vec<u8> = (0..sides.len()).map(half).collect();
    let mut ring: Vec<usize> = (0..sides.len()).collect();
    ring.sort_by(|&i, &j| {
        halves[i].cmp(&halves[j]).then_with(|| {
            if halves[i] % 2 == 1 && i != j {
                match cross(i, j) {
                    1 => Ordering::Less,
                    -1 => Ordering::Greater,
                    _ => i.cmp(&j),
                }
            } else {
                i.cmp(&j)
            }
        })
    });
    let open = ring.iter().map(|&i| sigma[i] == -1).collect();
    (ring.iter().map(|&i| sides[i]).collect(), open)
}

/// The sides round an edge (`ring`, counter-clockwise) paired like parentheses: each face that
/// opens a wedge of solid with the next face that closes one, so each pair bounds a wedge of solid
/// and bodies touching along the edge come apart; or, `across`, each closing face with the next
/// opening one, so each pair bounds an empty wedge and the bodies stay joined along the edge.
/// Matching starts after the lowest prefix sum, so every closing side finds an opening one.
fn match_round(ring: &[u32], open: &[bool], across: bool) -> Vec<(u32, u32)> {
    let opens = |k: usize| open[k] != across;
    let (mut sum, mut low, mut first) = (0i32, 0i32, 0usize);
    for k in 0..ring.len() {
        sum += if opens(k) { 1 } else { -1 };
        if sum < low {
            low = sum;
            first = k + 1;
        }
    }
    let mut stack = Vec::new();
    let mut pairs = Vec::with_capacity(ring.len() / 2);
    for k in 0..ring.len() {
        let at = (first + k) % ring.len();
        if opens(at) {
            stack.push(ring[at]);
        } else if let Some(o) = stack.pop() {
            pairs.push((o, ring[at]));
        }
    }
    pairs
}

// ---------------------------------------------------------------------------------------------
// 4. Rounding to the grid

/// The spacing of f32 values at x.
fn ulp_f32(x: f64) -> f64 {
    let x = (x.abs() as f32).max(f32::MIN_POSITIVE);
    (f32::from_bits(x.to_bits() + 1) - x) as f64
}

/// The grid every vertex of a wrapped surface lies on, in mm: the reference tool rounds every
/// vertex of its results to it.
const GRID: f64 = 0.002;

/// The grid cell a point rounds to: its centre's coordinates in grid steps.
fn grid_cell(p: P3) -> [i64; 3] {
    p.map(|x| (x / GRID).round() as i64)
}

fn grid_point(c: [i64; 3]) -> P3 {
    c.map(|k| k as f64 * GRID)
}

/// True when segment a–b meets the grid cell centred on c (slab test).
fn segment_meets_cell(a: P3, b: P3, c: P3) -> bool {
    let half = GRID / 2.0;
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for k in 0..3 {
        let d = b[k] - a[k];
        let (near, far) = (c[k] - half - a[k], c[k] + half - a[k]);
        if d == 0.0 {
            if near > 0.0 || far < 0.0 {
                return false;
            }
            continue;
        }
        let (t0, t1) = (near / d, far / d);
        lo = lo.max(t0.min(t1));
        hi = hi.min(t0.max(t1));
        if lo > hi {
            return false;
        }
    }
    true
}

/// The surface with every point rounded to the nearest f32 instead of the grid, faces left exactly
/// flat zipped into their neighbours; as `round_surface` returns it.
fn in_f32(surf: &Surface) -> (Mesh, Vec<u32>, Vec<u32>) {
    let mesh = Mesh {
        positions: surf
            .points
            .iter()
            .flat_map(|p| p.map(|x| x as f32))
            .collect(),
        indices: surf.tris.as_flattened().to_vec(),
    };
    let vertex: Vec<u32> = (0..surf.points.len() as u32).collect();
    match zip_collinear(&mesh) {
        Some((zipped, map)) => (zipped, map, vertex),
        None => (mesh, (0..surf.tris.len() as u32).collect(), vertex),
    }
}

/// Rounds of local re-wrapping and mending at most, while each resolves an eighth of the spots or
/// more.
const RESOLVE_ROUNDS: usize = 4;

/// Resolves the spots of a whole pass's result where it can (`resolve_round`), in rounds while each
/// resolves an eighth of them or more. Crossings the grid cannot resolve, where faces lie closer
/// together than it can tell apart, get one more round with their spots' new points rounded to f32
/// instead.
fn resolve_spots(mut w: Wrapped, control: &mut Control) -> RepairResult<Wrapped> {
    for _ in 0..RESOLVE_ROUNDS {
        let before = w.3.len();
        w = resolve_round(w, true, control)?;
        if w.3.is_empty() || w.3.len() * 8 > before * 7 {
            break;
        }
    }
    if w.3.iter().any(|&(a, b)| a != b) {
        w = resolve_round(w, false, control)?;
    }
    Ok(w)
}

/// One round over the spots: they are wrapped again on their own (`rewrap_spots`) while that
/// resolves an eighth of them or more, once over wider regions when a round gets nowhere; the
/// crossings left are then mended on the grid (`mend_on_grid`).
fn resolve_round(mut w: Wrapped, on_grid: bool, control: &mut Control) -> RepairResult<Wrapped> {
    control.stage("Wrapping the crossings left again");
    let mut rings = SPOT_RINGS;
    let mut failed = std::collections::HashSet::new();
    for _ in 0..LOCAL_PASSES {
        if w.3.is_empty() {
            break;
        }
        match rewrap_spots(&w.0, &w.1, &w.3, rings, on_grid, &mut failed, control)? {
            Some(better) => {
                let enough = better.3.len() * 8 <= w.3.len() * 7;
                w = better;
                if !enough {
                    break;
                }
            }
            None if rings == SPOT_RINGS => rings *= 2,
            None => break,
        }
    }
    if w.3.iter().any(|&(a, b)| a != b)
        && let Some(better) = mend_spots(&w, control)?
    {
        w = better;
    }
    Ok(w)
}

/// The crossings of `w` mended on the grid (`mend_on_grid`), when that leaves fewer spots.
fn mend_spots(w: &Wrapped, control: &mut Control) -> RepairResult<Option<Wrapped>> {
    let (mesh, origin, problems, spots) = w;
    control.stage("Mending what is left");
    let (mended, map, left) = mend_on_grid(mesh, spots, control)?;
    let crossing = left.iter().filter(|&&(a, b)| a != b).count();
    if left.len() >= spots.len() {
        return Ok(None);
    }
    let checked = Check {
        intersecting: crossing,
        ..*problems
    };
    Ok(Some((mended, follow(origin, &map), checked, left)))
}

/// The surface on the grid, as a closed manifold mesh: `snap_round`, then the non-manifold edges its
/// collapses made split (`split_edges::split_non_manifold`: each fan gets its own copies of the
/// edge's vertices), then faces left exactly flat zipped into their neighbours. The mesh, per
/// triangle the surface triangle it came from, and per vertex the surface vertex it stands for.
///
/// Where rounding stacked layers of the surface into the same cells, a crowd of sides round an edge
/// can join the fans round both of its ends however they pair, and splitting unglues them, which
/// cuts the surface open there (a face or a few come loose, each side of the cut at the same
/// position as one running the other way). Those cuts are welded shut again (`weld_seams`): the
/// edge stays non-manifold, but every edge stays balanced, so the winding numbers stay defined and
/// the faces round it are spots that are wrapped again on their own.
fn round_surface(
    surf: &Surface,
    keep: &(impl Fn(u32) -> bool + Sync),
    control: &mut Control,
) -> RepairResult<(Mesh, Vec<u32>, Vec<u32>)> {
    let (rounded, origin) = snap_round(surf, keep, &mut control.sub(0.0, 0.6))?;
    let mut split = split_edges::split_non_manifold(&rounded, &[], &mut control.sub(0.6, 0.9))?;
    if let Some(welded) = weld_seams(&split.mesh) {
        split.mesh = welded;
    }
    // Triangles keep their order and corners their place: a copy stands for its corner's vertex.
    let mut vertex: Vec<u32> = (0..split.mesh.vertex_count() as u32).collect();
    for (&v, &was) in split.mesh.indices.iter().zip(&rounded.indices) {
        vertex[v as usize] = was;
    }
    Ok(match zip_collinear(&split.mesh) {
        Some((zipped, map)) => (zipped, follow(&origin, &map), vertex),
        None => (split.mesh, origin, vertex),
    })
}

/// The surface snap-rounded to the grid (`GRID`): every vertex goes to the centre of its cell, and
/// features smaller than a cell collapse instead of crossing. As in snap rounding in the plane,
/// every cell with a vertex is hot: an edge that passes through a hot cell (one where neither of its
/// ends lies) is routed through that cell's centre. Each triangle then becomes the closed walk
/// through the cells its edges pass, with every part that goes and comes back the same way taken
/// out: a sliver whose edges all run through the same cells leaves nothing, and its neighbours
/// meet across it. What is left of the walk is filled with triangles in the plane the triangle lay
/// in. Triangles over the same corners facing opposite ways cancel in pairs. Every step keeps the
/// surface closed (as many sides one way as the other along every edge), but vertices of different
/// sheets in one cell become one, which can pinch it. `keep` vertices stay themselves and an edge
/// between two of them is never routed elsewhere, so a region wrapped again fits back. The mesh
/// (over the surface's vertices, unused ones left in place) and, per triangle, the surface triangle
/// it came from.
fn snap_round(
    surf: &Surface,
    keep: &(impl Fn(u32) -> bool + Sync),
    control: &mut Control,
) -> RepairResult<(Mesh, Vec<u32>)> {
    let n = surf.points.len();
    let point = |v: u32| surf.points[v as usize];
    let tris = &surf.tris;
    let cell: Vec<[i64; 3]> = surf.points.par_iter().map(|&p| grid_cell(p)).collect();
    // Every vertex becomes the one standing for its cell (a kept one if there is one, else the
    // lowest), except kept vertices, which stay themselves.
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.par_sort_unstable_by_key(|&v| (cell[v as usize], !keep(v), v));
    let mut stand = vec![0u32; n];
    let mut firsts: Vec<u32> = Vec::new();
    for group in order.chunk_by(|&a, &b| cell[a as usize] == cell[b as usize]) {
        firsts.push(group[0]);
        for &v in group {
            stand[v as usize] = if keep(v) { v } else { group[0] };
        }
    }
    drop(order);
    let boxes: Vec<Aabb> = tris
        .par_iter()
        .map(|t| grown(box_of(&t.map(point)), GRID / 2.0))
        .collect();
    let tree = Bvh::new(&boxes);
    control.check()?;
    let cancel = control.cancel_flag();
    // The hot cells each edge passes: (edge lo, edge hi, the cell's vertex, parameter from lo).
    let mut on_edges: Vec<(u32, u32, u32, f64)> = firsts
        .par_iter()
        .flat_map_iter(|&v| {
            let mut found = Vec::new();
            if cancel.load(Atomic::Relaxed) {
                return found;
            }
            let c = cell[v as usize];
            let centre = grid_point(c);
            tree.query(&[centre, centre], |t| {
                let tri = tris[t as usize];
                if tri.iter().any(|&w| cell[w as usize] == c) {
                    return;
                }
                for k in 0..3 {
                    let (a, b) = (tri[k], tri[(k + 1) % 3]);
                    let (pa, pb) = (point(a), point(b));
                    if (keep(a) && keep(b)) || !segment_meets_cell(pa, pb, centre) {
                        continue;
                    }
                    let d = exact::sub(pb, pa);
                    let s = exact::dot(exact::sub(centre, pa), d) / exact::dot(d, d);
                    let (lo, hi, s) = if a < b { (a, b, s) } else { (b, a, 1.0 - s) };
                    found.push((lo, hi, v, s));
                }
            });
            found
        })
        .collect();
    control.check()?;
    on_edges.par_sort_unstable_by(|x, y| {
        (x.0, x.1)
            .cmp(&(y.0, y.1))
            .then(x.3.total_cmp(&y.3))
            .then(x.2.cmp(&y.2))
    });
    on_edges.dedup_by(|x, y| (x.0, x.1, x.2) == (y.0, y.1, y.2));
    control.progress(0.3);
    let rounded = |v: u32| grid_point(cell[v as usize]);
    let made: Vec<([u32; 3], u32)> = (0..tris.len())
        .into_par_iter()
        .flat_map_iter(|t| {
            let tri = tris[t];
            let mut walk: Vec<u32> = Vec::new();
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                walk.push(stand[a as usize]);
                let list = points_on(&on_edges, a, b);
                if a < b {
                    walk.extend(list.iter().map(|x| x.2));
                } else {
                    walk.extend(list.iter().rev().map(|x| x.2));
                }
            }
            let mut out = Vec::new();
            for lp in simple_loops(&reduced_walk(&walk)) {
                for f in fill_walk(&lp, &rounded, surf.normals[t]) {
                    out.push((f, t as u32));
                }
            }
            out
        })
        .collect();
    control.check()?;
    control.progress(0.8);
    // Opposite copies over the same corners cancel in pairs.
    let mut keys: Vec<([u32; 3], bool, u32)> = made
        .par_iter()
        .enumerate()
        .map(|(i, (t, _))| {
            let (key, even) = sorted(*t);
            (key, even, i as u32)
        })
        .collect();
    keys.par_sort_unstable();
    let mut gone = vec![false; made.len()];
    for group in keys.chunk_by(|a, b| a.0 == b.0) {
        let odd = group.partition_point(|k| !k.1);
        let pairs = odd.min(group.len() - odd);
        for k in group[odd - pairs..odd]
            .iter()
            .chain(&group[group.len() - pairs..])
        {
            gone[k.2 as usize] = true;
        }
    }
    let mut indices = Vec::with_capacity(made.len() * 3);
    let mut origin = Vec::with_capacity(made.len());
    for (i, (f, t)) in made.into_iter().enumerate() {
        if !gone[i] {
            indices.extend_from_slice(&f);
            origin.push(t);
        }
    }
    let positions: Vec<f32> = cell
        .iter()
        .flat_map(|&c| grid_point(c).map(|x| x as f32))
        .collect();
    Ok((Mesh { positions, indices }, origin))
}

/// A closed walk (its stops in order, the last joined back to the first) without the parts that go
/// and come back the same way: repeated stops merge, and a spur x → y → x is cut back to x, until
/// none is left. Empty when fewer than three stops are left.
fn reduced_walk(walk: &[u32]) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::with_capacity(walk.len());
    for &v in walk {
        if out.last() == Some(&v) {
            continue;
        }
        if out.len() >= 2 && out[out.len() - 2] == v {
            out.pop();
            continue;
        }
        out.push(v);
    }
    // Where the walk closes.
    loop {
        let n = out.len();
        if (n >= 2 && out[0] == out[n - 1]) || (n >= 3 && out[n - 2] == out[0]) {
            out.pop();
        } else if n >= 3 && out[n - 1] == out[1] {
            out.remove(0);
        } else {
            break;
        }
    }
    if out.len() < 3 { Vec::new() } else { out }
}

/// A closed walk split into simple loops (no stop twice) of three stops or more: a stop it reaches
/// again closes a loop there.
fn simple_loops(walk: &[u32]) -> Vec<Vec<u32>> {
    let mut loops = Vec::new();
    let mut path: Vec<u32> = Vec::with_capacity(walk.len());
    for &v in walk {
        match path.iter().position(|&w| w == v) {
            Some(at) => {
                let lp: Vec<u32> = path.drain(at..).collect();
                if lp.len() >= 3 {
                    loops.push(lp);
                }
                path.push(v);
            }
            None => path.push(v),
        }
    }
    if path.len() >= 3 {
        loops.push(path);
    }
    loops
}

/// Triangles filling a simple loop over points `point`, facing the way of `normal` as the loop
/// runs: ear clipping in the plane `normal` points out of most, or a fan where that projection
/// folds.
fn fill_walk(lp: &[u32], point: &impl Fn(u32) -> P3, normal: P3) -> Vec<[u32; 3]> {
    if lp.len() == 3 {
        return vec![[lp[0], lp[1], lp[2]]];
    }
    let axis = (0..3)
        .max_by(|&x, &y| normal[x].abs().total_cmp(&normal[y].abs()))
        .unwrap_or(2);
    let flat: Vec<[f64; 2]> = lp.iter().map(|&v| exact::project(point(v), axis)).collect();
    let turn = if normal[axis] > 0.0 { 1 } else { -1 };
    ear_clip(lp, &flat, turn).unwrap_or_else(|| {
        (1..lp.len() - 1)
            .map(|i| [lp[0], lp[i], lp[i + 1]])
            .collect()
    })
}

// ---------------------------------------------------------------------------------------------
// 5. Mending on the grid

/// Rounds of mending at most, the rings a spot's region may grow by when it is patched, and the
/// most faces such a region may have (filling its hole takes time quadratic in its border).
const FIX_ROUNDS: usize = 4;
/// Faces or vertices mended between two looks at the cancel flag (a few milliseconds' work).
const FIX_CHECK_EVERY: usize = 64;
const PATCH_GROWTH: usize = 4;
const PATCH_MAX: usize = 4000;

/// How far a nudge may put a point from where it started, in grid steps, and how many positions it
/// tries at most (the nearest).
const NUDGE_REACH: f64 = 2.5;
/// The most triangle pairs one mending tests for crossing, per face of the mesh and at least: where
/// faces lie stacked so densely that each try tests thousands, the spots left go to the last
/// resort (`rebuilt`) instead.
const MEND_TESTS: u64 = 200;
const MEND_TESTS_MIN: u64 = 20_000_000;
const NUDGE_OPTIONS: usize = 64;

/// The cells faces beside the tree are filed by, in grid steps, and how many cells a box may cover
/// before it goes on the list of big ones.
const LOOSE_CELL: f64 = 1024.0;
const LOOSE_SPAN: u64 = 512;

/// The faces round every vertex, as `start` offsets into a list.
fn vertex_rings(count: usize, tris: &[[u32; 3]]) -> (Vec<u32>, Vec<u32>) {
    let mut start = vec![0u32; count + 1];
    for tri in tris {
        for &v in tri {
            start[v as usize + 1] += 1;
        }
    }
    for v in 0..count {
        start[v + 1] += start[v];
    }
    let mut fill = start.clone();
    let mut around = vec![0u32; tris.len() * 3];
    for (t, tri) in tris.iter().enumerate() {
        for &v in tri {
            around[fill[v as usize] as usize] = t as u32;
            fill[v as usize] += 1;
        }
    }
    (start, around)
}

/// The box grown by `by` on every side.
fn grown(mut b: Aabb, by: f64) -> Aabb {
    for x in b[0].iter_mut() {
        *x -= by;
    }
    for x in b[1].iter_mut() {
        *x += by;
    }
    b
}

/// The box round some points.
fn box_of(points: &[P3]) -> Aabb {
    let mut b = bvh::EMPTY;
    for &p in points {
        bvh::include(&mut b, p);
    }
    b
}

/// A surface on the grid while its spots are mended: faces that cross, or that a change would make
/// flat or turn over. Changes stay local and so do the checks: a tree over the faces as they were
/// at the start finds the faces near any face that stayed inside its box there; faces made later,
/// or moved out of their boxes, are filed beside it by cells. Every change is checked before it
/// stays: the faces it makes are not flat, face the way they faced at the start and cross nothing,
/// so the surface only gets better.
struct Fix {
    pos: Vec<[f32; 3]>,
    /// Per vertex: where it was when the mending started.
    exact: Vec<P3>,
    tris: Vec<[u32; 3]>,
    alive: Vec<bool>,
    /// Per face: the face of the rounded mesh whose plane it keeps (NEW for patch faces), and the
    /// normal it must keep facing.
    origin: Vec<u32>,
    normal: Vec<P3>,
    /// The faces round each vertex as rounding left them, and the lists of vertices that changed
    /// since (both may hold faces that no longer use the vertex; `ring` filters them).
    start: Vec<u32>,
    around: Vec<u32>,
    changed: std::collections::HashMap<u32, Vec<u32>>,
    tree: Bvh,
    boxes: Vec<Aabb>,
    /// The faces beside the tree, in the order they went there, and per face whether it is one.
    /// They are filed by cells of `cell` across under the cells their boxes cover whenever they
    /// change (entries they left are skipped), or in `big` when that is many cells.
    loose: Vec<u32>,
    is_loose: Vec<bool>,
    cells: std::collections::HashMap<[i64; 3], Vec<u32>>,
    big: Vec<u32>,
    /// Per face filed: the cells it was last filed under (lowest, highest), whose lists still hold
    /// it, so filing it there again adds nothing.
    filed: std::collections::HashMap<u32, ([i64; 3], [i64; 3])>,
    cell: f64,
    unit: f64,
    /// Triangle pairs tested for crossing so far (`MEND_TESTS`).
    tests: std::sync::atomic::AtomicU64,
}

impl Fix {
    fn new(mesh: &Mesh) -> Fix {
        let tris: Vec<[u32; 3]> = mesh.indices.as_chunks::<3>().0.to_vec();
        let pos: Vec<[f32; 3]> = mesh.positions.as_chunks::<3>().0.to_vec();
        let (start, around) = vertex_rings(pos.len(), &tris);
        let boxes: Vec<Aabb> = tris
            .par_iter()
            .map(|t| box_of(&t.map(|v| pos[v as usize].map(f64::from))))
            .collect();
        let normal: Vec<P3> = tris
            .par_iter()
            .map(|t| {
                let [a, b, c] = t.map(|v| pos[v as usize].map(f64::from));
                exact::normal(a, b, c)
            })
            .collect();
        let tree = Bvh::new(&boxes);
        let n = tris.len();
        Fix {
            exact: pos.iter().map(|p| p.map(f64::from)).collect(),
            pos,
            alive: vec![true; n],
            origin: (0..n as u32).collect(),
            normal,
            tris,
            start,
            around,
            changed: Default::default(),
            tree,
            boxes,
            loose: Vec::new(),
            is_loose: vec![false; n],
            cells: Default::default(),
            big: Vec::new(),
            filed: Default::default(),
            cell: LOOSE_CELL * GRID,
            unit: GRID,
            tests: Default::default(),
        }
    }

    fn point(&self, v: u32) -> P3 {
        self.pos[v as usize].map(f64::from)
    }

    fn corners(&self, t: u32) -> [P3; 3] {
        self.tris[t as usize].map(|v| self.point(v))
    }

    /// The live faces round vertex v.
    fn ring(&self, v: u32) -> Vec<u32> {
        let list: &[u32] = match self.changed.get(&v) {
            Some(list) => list,
            None if (v as usize) + 1 < self.start.len() => {
                &self.around[self.start[v as usize] as usize..self.start[v as usize + 1] as usize]
            }
            None => &[],
        };
        list.iter()
            .copied()
            .filter(|&t| self.alive[t as usize] && self.tris[t as usize].contains(&v))
            .collect()
    }

    /// Adds faces to the ring of vertex v.
    fn add_to_ring(&mut self, v: u32, faces: &[u32]) {
        let mut list = self.ring(v);
        list.extend_from_slice(faces);
        list.sort_unstable();
        list.dedup();
        self.changed.insert(v, list);
    }

    /// Flat, or turned over against the normal it must face.
    fn bad_shape(&self, t: u32) -> bool {
        thinness(self.corners(t), self.normal[t as usize]) <= 0.0
    }

    /// The cells a box covers, and how many.
    fn cell_range(&self, b: &Aabb) -> ([i64; 3], [i64; 3], u64) {
        let lo = b[0].map(|x| (x / self.cell).floor() as i64);
        let hi = b[1].map(|x| (x / self.cell).floor() as i64);
        let count = (0..3).map(|k| (hi[k] - lo[k] + 1).max(0) as u64).product();
        (lo, hi, count)
    }

    /// Files loose face t under the cells its box covers now.
    fn file(&mut self, t: u32) {
        let (lo, hi, count) = self.cell_range(&box_of(&self.corners(t)));
        if self.filed.insert(t, (lo, hi)) == Some((lo, hi)) {
            return;
        }
        if count > LOOSE_SPAN {
            self.big.push(t);
            return;
        }
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    self.cells.entry([x, y, z]).or_default().push(t);
                }
            }
        }
    }

    /// Calls f for every live face whose box may meet box b (some more than once).
    fn near(&self, b: &Aabb, mut f: impl FnMut(u32)) {
        self.tree.query(b, |t| {
            if self.alive[t as usize] && !self.is_loose[t as usize] {
                f(t);
            }
        });
        if self.loose.is_empty() {
            return;
        }
        let mut check = |t: u32| {
            if self.alive[t as usize]
                && self.is_loose[t as usize]
                && bvh::overlap(&box_of(&self.corners(t)), b)
            {
                f(t);
            }
        };
        let (lo, hi, count) = self.cell_range(b);
        if count > LOOSE_SPAN {
            self.loose.iter().for_each(|&t| check(t));
            return;
        }
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    if let Some(list) = self.cells.get(&[x, y, z]) {
                        list.iter().for_each(|&t| check(t));
                    }
                }
            }
        }
        self.big.iter().for_each(|&t| check(t));
    }

    /// Face t crosses a live face.
    fn crosses(&self, t: u32) -> bool {
        let p = self.corners(t);
        let mut hit = false;
        self.near(&box_of(&p), |u| {
            if !hit && u != t {
                self.tests.fetch_add(1, Atomic::Relaxed);
            }
            hit = hit || (u != t && tritri::intersect(&p, &self.corners(u)));
        });
        hit
    }

    fn broken(&self, t: u32) -> bool {
        self.bad_shape(t) || self.crosses(t)
    }

    /// Face t changed: into the short list when it left the box the tree holds for it. Returns
    /// whether it went in, so a change that is undone can take it out again.
    fn moved(&mut self, t: u32) -> bool {
        let i = t as usize;
        if self.is_loose[i] {
            self.file(t);
            return false;
        }
        let b = box_of(&self.corners(t));
        let inside = i < self.boxes.len()
            && (0..3).all(|k| b[0][k] >= self.boxes[i][0][k] && b[1][k] <= self.boxes[i][1][k]);
        if inside {
            return false;
        }
        self.is_loose[i] = true;
        self.loose.push(t);
        self.file(t);
        true
    }

    /// Takes the faces that went into the short list after its length was `mark` out again.
    fn unloose(&mut self, mark: usize) {
        for &t in &self.loose[mark..] {
            self.is_loose[t as usize] = false;
        }
        self.loose.truncate(mark);
    }

    /// The faces are well shaped and cross nothing.
    fn all_good(&self, faces: &[u32]) -> bool {
        if !faces
            .iter()
            .all(|&t| !self.bad_shape(t) && !self.crosses(t))
        {
            return false;
        }
        // Changed faces the tree still holds where they were can miss each other there.
        let corners: Vec<[P3; 3]> = faces.iter().map(|&t| self.corners(t)).collect();
        (0..corners.len()).all(|i| (0..i).all(|j| !tritri::intersect(&corners[i], &corners[j])))
    }

    /// Moves vertex v to the grid point nearest where it started, among those within `NUDGE_REACH`
    /// steps of it (the grid two steps round where it is, and nearby vertices' positions), at which
    /// every face round it is well shaped and crosses nothing. True when the faces round it are
    /// fine (moved or not).
    fn nudge(&mut self, v: u32) -> bool {
        let faces = self.ring(v);
        if self.all_good(&faces) {
            return true;
        }
        let x = self.exact[v as usize];
        let here = self.pos[v as usize];
        let reach = NUDGE_REACH * self.unit;
        let within = |o: &[f32; 3]| (0..3).all(|k| (o[k] as f64 - x[k]).abs() <= reach);
        let mut options: Vec<[f32; 3]> = Vec::with_capacity(48);
        let at = grid_cell(here.map(f64::from));
        for dx in -2..=2 {
            for dy in -2..=2 {
                for dz in -2..=2 {
                    let c = [at[0] + dx, at[1] + dy, at[2] + dz];
                    options.push(grid_point(c).map(|x| x as f32));
                }
            }
        }
        let b = grown(box_of(&[x]), reach);
        self.near(&b, |t| {
            for &w in &self.tris[t as usize] {
                if w != v {
                    options.push(self.pos[w as usize]);
                }
            }
        });
        options.retain(|o| *o != here && within(o));
        let distance = |o: &[f32; 3]| (0..3).map(|k| (o[k] as f64 - x[k]).powi(2)).sum::<f64>();
        options.sort_by(|p, q| {
            distance(p)
                .total_cmp(&distance(q))
                .then(p.partial_cmp(q).unwrap_or(Ordering::Equal))
        });
        options.dedup();
        options.truncate(NUDGE_OPTIONS);
        for o in options {
            self.pos[v as usize] = o;
            if self.all_good(&faces) {
                for &t in &faces {
                    self.moved(t);
                }
                return true;
            }
        }
        self.pos[v as usize] = here;
        false
    }

    /// Collapses the edge from vertex v to its neighbour u: the two faces on the edge go, the
    /// others round v take u instead. Only when the surface stays manifold (v and u share exactly
    /// the two neighbours across the edge) and no face repeats.
    fn collapse(&mut self, v: u32, u: u32) -> bool {
        if v == u {
            return false;
        }
        let around_v = self.ring(v);
        let on_edge: Vec<u32> = around_v
            .iter()
            .copied()
            .filter(|&t| self.tris[t as usize].contains(&u))
            .collect();
        if on_edge.len() != 2 {
            return false;
        }
        let apex = |t: u32| {
            self.tris[t as usize]
                .iter()
                .copied()
                .find(|&w| w != u && w != v)
                .unwrap_or(u)
        };
        if apex(on_edge[0]) == apex(on_edge[1]) {
            return false;
        }
        let neighbours = |w: u32| {
            let mut n: Vec<u32> = self
                .ring(w)
                .iter()
                .flat_map(|&t| self.tris[t as usize])
                .filter(|&x| x != w)
                .collect();
            n.sort_unstable();
            n.dedup();
            n
        };
        let (nu, nv) = (neighbours(u), neighbours(v));
        if nv.iter().filter(|x| nu.binary_search(x).is_ok()).count() != 2 {
            return false;
        }
        let moving: Vec<u32> = around_v
            .iter()
            .copied()
            .filter(|t| !on_edge.contains(t))
            .collect();
        let old: Vec<[u32; 3]> = moving.iter().map(|&t| self.tris[t as usize]).collect();
        let made: Vec<[u32; 3]> = old
            .iter()
            .map(|f| f.map(|x| if x == v { u } else { x }))
            .collect();
        let existing: Vec<[u32; 3]> = self
            .ring(u)
            .iter()
            .map(|&t| sorted(self.tris[t as usize]).0)
            .collect();
        if made.iter().any(|&f| existing.contains(&sorted(f).0)) {
            return false;
        }
        let mark = self.loose.len();
        for &t in &on_edge {
            self.alive[t as usize] = false;
        }
        for (&t, &f) in moving.iter().zip(&made) {
            self.tris[t as usize] = f;
        }
        for &t in &moving {
            self.moved(t);
        }
        if !self.all_good(&moving) {
            for (&t, &f) in moving.iter().zip(&old) {
                self.tris[t as usize] = f;
            }
            for &t in &on_edge {
                self.alive[t as usize] = true;
            }
            self.unloose(mark);
            return false;
        }
        self.add_to_ring(u, &moving);
        true
    }

    /// Flips the longest side of face t with the face across it, when t is a cap (its apex over
    /// the inside of that side). Nothing moves; the two new faces must face both old faces' way.
    fn flip(&mut self, t: u32) -> bool {
        let tri = self.tris[t as usize];
        let p = self.corners(t);
        let side = [0, 1, 2].map(|k| exact::length(exact::sub(p[(k + 1) % 3], p[k])));
        let k = (0..3)
            .max_by(|&x, &y| side[x].total_cmp(&side[y]))
            .unwrap_or(0);
        let (a, b, m) = (tri[k], tri[(k + 1) % 3], tri[(k + 2) % 3]);
        let Some(n) = self.ring(b).into_iter().find(|&f| {
            f != t && {
                let x = self.tris[f as usize];
                (0..3).any(|j| x[j] == b && x[(j + 1) % 3] == a)
            }
        }) else {
            return false;
        };
        let d = self.tris[n as usize]
            .iter()
            .copied()
            .find(|&w| w != a && w != b)
            .unwrap_or(a);
        if d == m
            || self
                .ring(m)
                .iter()
                .any(|&f| self.tris[f as usize].contains(&d))
        {
            return false;
        }
        let dir = exact::sub(self.point(b), self.point(a));
        let s = exact::dot(exact::sub(self.point(m), self.point(a)), dir) / exact::dot(dir, dir);
        if !(s > 0.0 && s < 1.0) {
            return false;
        }
        let (old_t, old_n) = (self.tris[t as usize], self.tris[n as usize]);
        let (normal_t, normal_n) = (self.normal[t as usize], self.normal[n as usize]);
        let mark = self.loose.len();
        self.tris[t as usize] = [b, m, d];
        self.tris[n as usize] = [m, a, d];
        self.moved(t);
        self.moved(n);
        let faces_both = [t, n].iter().all(|&f| {
            let q = self.corners(f);
            let normal = exact::normal(q[0], q[1], q[2]);
            exact::dot(normal, normal_t) > 0.0 && exact::dot(normal, normal_n) > 0.0
        });
        if !faces_both || !self.all_good(&[t, n]) {
            self.tris[t as usize] = old_t;
            self.tris[n as usize] = old_n;
            self.unloose(mark);
            return false;
        }
        self.origin[t as usize] = self.origin[n as usize];
        self.normal[t as usize] = normal_n;
        self.add_to_ring(m, &[n]);
        self.add_to_ring(d, &[t]);
        true
    }

    /// Cuts the region (faces) out and fills each hole it leaves anew from its border
    /// (`fill_loop`; a fan's centre goes on the grid). The patch faces must not be flat, cross
    /// nothing (each other included) and make no edge the surface round has already.
    fn patch(&mut self, region: &[u32]) -> bool {
        let in_region = |t: u32| region.binary_search(&t).is_ok();
        // The sides of the faces round the region across its faces' sides, as they run.
        let mut border: Vec<(u32, u32)> = Vec::new();
        let mut own: Vec<(u32, u32)> = Vec::new();
        for &t in region {
            let tri = self.tris[t as usize];
            for k in 0..3 {
                let (u, v) = (tri[k], tri[(k + 1) % 3]);
                own.push((u, v));
                let across: Vec<u32> = self
                    .ring(u)
                    .into_iter()
                    .filter(|&f| {
                        f != t && {
                            let x = self.tris[f as usize];
                            (0..3).any(|j| x[j] == v && x[(j + 1) % 3] == u)
                        }
                    })
                    .collect();
                match across.as_slice() {
                    [] => return false,
                    [f] if !in_region(*f) => border.push((v, u)),
                    [_] => {}
                    _ => return false,
                }
            }
        }
        own.sort_unstable();
        let count = self.pos.len();
        let mut extra: Vec<f32> = Vec::new();
        let mut made: Vec<[u32; 3]> = Vec::new();
        for lp in loops_of(border, count) {
            let point = |v: u32| {
                if (v as usize) < count {
                    self.point(v)
                } else {
                    let k = (v as usize - count) * 3;
                    [extra[k], extra[k + 1], extra[k + 2]].map(f64::from)
                }
            };
            let next = (count + extra.len() / 3) as u32;
            match fill_loop_with(&point, next, &lp) {
                Some((faces, center)) => {
                    made.extend(faces);
                    if let Some(c) = center {
                        let on_grid = grid_point(grid_cell(c.map(f64::from)));
                        extra.extend_from_slice(&on_grid.map(|x| x as f32));
                    }
                }
                None => return false,
            }
        }
        let position = |v: u32| {
            if (v as usize) < count {
                self.point(v)
            } else {
                let k = (v as usize - count) * 3;
                [extra[k], extra[k + 1], extra[k + 2]].map(f64::from)
            }
        };
        let faces: Vec<[P3; 3]> = made.iter().map(|f| f.map(position)).collect();
        for (i, f) in made.iter().enumerate() {
            let p = faces[i];
            if exact::collinear(p[0], p[1], p[2]) {
                return false;
            }
            for k in 0..3 {
                let (a, b) = (f[k], f[(k + 1) % 3]);
                if own.binary_search(&(a, b)).is_ok() || a as usize >= count || b as usize >= count
                {
                    continue;
                }
                if self
                    .ring(a)
                    .iter()
                    .any(|&u| !in_region(u) && self.tris[u as usize].contains(&b))
                {
                    return false;
                }
            }
            let mut hit = false;
            self.near(&box_of(&p), |u| {
                hit = hit || (!in_region(u) && tritri::intersect(&p, &self.corners(u)));
            });
            if hit || faces[..i].iter().any(|q| tritri::intersect(&p, q)) {
                return false;
            }
        }
        // It holds: the region goes, the patch comes.
        for &t in region {
            self.alive[t as usize] = false;
        }
        for c in extra.chunks(3) {
            let c = [c[0], c[1], c[2]];
            self.pos.push(c);
            self.exact.push(c.map(f64::from));
        }
        for (f, p) in made.iter().zip(&faces) {
            let t = self.tris.len() as u32;
            self.tris.push(*f);
            self.alive.push(true);
            self.origin.push(NEW);
            self.normal.push(exact::normal(p[0], p[1], p[2]));
            self.is_loose.push(true);
            self.loose.push(t);
            self.file(t);
            for &v in f {
                self.add_to_ring(v, &[t]);
            }
        }
        true
    }

    /// The spot round broken faces: they and the faces sharing a vertex with them, grown by
    /// `rings` more rings (sorted).
    fn region(&self, faces: &[u32], rings: usize) -> Vec<u32> {
        let mut out: Vec<u32> = faces.to_vec();
        for _ in 0..rings {
            let mut more: Vec<u32> = out
                .iter()
                .flat_map(|&t| self.tris[t as usize])
                .flat_map(|v| self.ring(v))
                .collect();
            more.extend_from_slice(&out);
            more.sort_unstable();
            more.dedup();
            out = more;
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Mends the crossings among `spots` (pairs that cross; (t, t) marks a face on an edge that is not
/// closed, which stays as it is) on the grid (`Fix`), spot by spot: vertices nudged to nearby grid
/// points, then thin faces flipped or collapsed (thinnest first, shortest edge first), then what
/// still crosses cut out and patched (a patch's centre on the grid too), in rounds until nothing
/// crosses or a round mends less than an eighth of what was broken. The mesh (patch vertices after
/// the others), per face the face of `mesh` it keeps the plane of (NEW for patches), and the spots
/// left, as they are numbered in the mesh.
fn mend_on_grid(mesh: &Mesh, spots: &[(u32, u32)], control: &mut Control) -> RepairResult<Mended> {
    let mut fix = Fix::new(mesh);
    let pairs: Vec<(u32, u32)> = spots.iter().copied().filter(|&(a, b)| a != b).collect();
    let pairs = &pairs[..];
    let mut seeds: Vec<u32> = pairs.iter().flat_map(|&(a, b)| [a, b]).collect();
    seeds.sort_unstable();
    seeds.dedup();
    let mut broken: Vec<u32> = seeds
        .par_iter()
        .copied()
        .filter(|&t| fix.broken(t))
        .collect();
    let still = |fix: &Fix, broken: &[u32]| -> Vec<u32> {
        broken
            .iter()
            .copied()
            .filter(|&t| fix.alive[t as usize] && fix.broken(t))
            .collect()
    };
    let budget = (MEND_TESTS * fix.tris.len() as u64).max(MEND_TESTS_MIN);
    let spent = |fix: &Fix| fix.tests.load(Atomic::Relaxed) > budget;
    'rounds: for round in 0..FIX_ROUNDS {
        control.check()?;
        if broken.is_empty() || spent(&fix) {
            break;
        }
        let before = broken.len();
        // 1. Nudges.
        let mut verts: Vec<u32> = broken.iter().flat_map(|&t| fix.tris[t as usize]).collect();
        verts.sort_unstable();
        verts.dedup();
        let share = |part: f64| (round as f64 + part) / FIX_ROUNDS as f64;
        let count = verts.len().max(1);
        for (i, v) in verts.into_iter().enumerate() {
            if i.is_multiple_of(FIX_CHECK_EVERY) {
                control.check()?;
                control.progress(share(i as f64 / count as f64 / 3.0));
                if spent(&fix) {
                    break 'rounds;
                }
            }
            fix.nudge(v);
        }
        broken = still(&fix, &broken);
        // 2. Flips and collapses, thinnest face first.
        let mut order: Vec<(f64, u32)> = broken
            .iter()
            .map(|&t| (thinness(fix.corners(t), fix.normal[t as usize]), t))
            .collect();
        order.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
        let count = order.len().max(1);
        for (i, (_, t)) in order.into_iter().enumerate() {
            if i.is_multiple_of(FIX_CHECK_EVERY) {
                control.check()?;
                control.progress(share((1.0 + i as f64 / count as f64) / 3.0));
                if spent(&fix) {
                    break 'rounds;
                }
            }
            if !fix.alive[t as usize] || !fix.broken(t) {
                continue;
            }
            if fix.flip(t) {
                continue;
            }
            let tri = fix.tris[t as usize];
            let p = fix.corners(t);
            let mut by_length = [0usize, 1, 2];
            let side = [0, 1, 2].map(|k| exact::length(exact::sub(p[(k + 1) % 3], p[k])));
            by_length.sort_by(|&x, &y| side[x].total_cmp(&side[y]));
            'edges: for k in by_length {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                for (v, u) in [(a, b), (b, a)] {
                    if fix.collapse(v, u) {
                        break 'edges;
                    }
                }
            }
        }
        broken = still(&fix, &broken);
        // 3. Patches round what is left, spot by spot: faces joined through corners, and faces
        // that crossed each other.
        let mut parent: Vec<u32> = (0..broken.len() as u32).collect();
        let mut seen: std::collections::HashMap<u32, u32> = Default::default();
        for (i, &t) in broken.iter().enumerate() {
            for v in fix.tris[t as usize] {
                match seen.get(&v) {
                    Some(&j) => join(&mut parent, i as u32, j),
                    None => {
                        seen.insert(v, i as u32);
                    }
                }
            }
        }
        for &(a, b) in pairs {
            if let (Ok(i), Ok(j)) = (broken.binary_search(&a), broken.binary_search(&b)) {
                join(&mut parent, i as u32, j as u32);
            }
        }
        let mut spots: Vec<(u32, u32)> = (0..broken.len() as u32)
            .map(|i| (root(&mut parent, i), broken[i as usize]))
            .collect();
        spots.sort_unstable();
        let count = spots.len().max(1);
        let mut done = 0;
        for spot in spots.chunk_by(|x, y| x.0 == y.0) {
            control.check()?;
            if spent(&fix) {
                break 'rounds;
            }
            control.progress(share((2.0 + done as f64 / count as f64) / 3.0));
            done += spot.len();
            let faces: Vec<u32> = spot.iter().map(|x| x.1).collect();
            if !faces.iter().all(|&t| fix.alive[t as usize])
                || !faces.iter().any(|&t| fix.broken(t))
            {
                continue;
            }
            for growth in 0..=PATCH_GROWTH {
                let region = fix.region(&faces, growth + 1);
                if region.len() > PATCH_MAX || fix.patch(&region) {
                    break;
                }
            }
        }
        broken = still(&fix, &broken);
        control.progress((round + 1) as f64 / FIX_ROUNDS as f64);
        // A round that mends less than an eighth of what was broken shows the rest is stuck.
        if broken.len() * 8 > before * 7 {
            break;
        }
    }
    // Every change was checked to cross nothing, so what still crosses are the pairs of faces
    // that did not change.
    let n = mesh.triangle_count();
    let same = |t: u32| fix.alive[t as usize] && fix.corners(t) == exact::corners(mesh, t);
    let mut new_id = vec![u32::MAX; fix.tris.len()];
    let mut indices = Vec::with_capacity(fix.tris.len() * 3);
    let mut origin = Vec::with_capacity(fix.tris.len());
    for (t, tri) in fix.tris.iter().enumerate() {
        if fix.alive[t] {
            new_id[t] = origin.len() as u32;
            indices.extend_from_slice(tri);
            origin.push(fix.origin[t]);
        }
    }
    // Faces on edges that are not closed stay as they were, if they are still there.
    let mut left: Vec<(u32, u32)> = spots
        .iter()
        .filter(|&&(a, b)| {
            (a as usize) < n
                && (b as usize) < n
                && if a == b {
                    fix.alive[a as usize]
                } else {
                    same(a) && same(b)
                }
        })
        .map(|&(a, b)| (new_id[a as usize], new_id[b as usize]))
        .collect();
    left.sort_unstable();
    let positions: Vec<f32> = fix.pos.iter().flatten().copied().collect();
    Ok((Mesh { positions, indices }, origin, left))
}

/// How thin a face is: the smaller of its shortest side and its height over its longest side;
/// zero when flat, negative when it faces against `normal` (the plane it lies in exactly).
fn thinness(p: [P3; 3], normal: P3) -> f64 {
    let sides = [0, 1, 2].map(|k| exact::length(exact::sub(p[(k + 1) % 3], p[k])));
    let longest = sides[0].max(sides[1]).max(sides[2]);
    let shortest = sides[0].min(sides[1]).min(sides[2]);
    if exact::collinear(p[0], p[1], p[2]) {
        return 0.0;
    }
    let n = exact::normal(p[0], p[1], p[2]);
    let height = exact::length(n) / longest.max(f64::MIN_POSITIVE);
    if exact::dot(n, normal) <= 0.0 {
        -height.max(f64::MIN_POSITIVE)
    } else {
        height.min(shortest)
    }
}

// ---------------------------------------------------------------------------------------------
// 6. What is left, wrapped again locally

/// Rounds of local re-wrapping at most, and the rings of faces round the crossing faces that a
/// region takes in.
const LOCAL_PASSES: usize = 6;
const SPOT_RINGS: usize = 1;

/// Rounds a region grows by along lines where bodies touch on its border, at most.
const PINCH_ROUNDS: usize = 64;

/// Vertex ids from this one up stand for a region's new points (`Local::extra`) until it is put
/// back.
const FRESH: u32 = 1 << 31;

/// A mended mesh, per face the face it keeps the plane of, and its spots.
type Mended = (Mesh, Vec<u32>, Vec<(u32, u32)>);

/// A wrapped mesh, the input triangle each of its faces came from, its check, and its spots: the
/// pairs of its faces that still cross, and (t, t) for each face t on an edge that is not closed.
type Wrapped = (Mesh, Vec<u32>, Check, Vec<(u32, u32)>);

/// A region (its faces) and what it became, when that crosses less.
type Improved = (Vec<u32>, Local);

/// A region of a surface wrapped again on its own (`rewrap_region`), ready to go back in place.
struct Local {
    /// Its faces over the surface's vertices, or new points (`FRESH` + index into `extra`).
    faces: Vec<[u32; 3]>,
    extra: Vec<[f32; 3]>,
    /// Per face, the input triangle it came from (NEW for patches).
    origin: Vec<u32>,
    /// Pairs of its faces crossing each other or a face outside the region (indices into `faces`
    /// for its own; the faces outside are not named).
    crossings: usize,
}

/// A surface being mended spot by spot: its faces, the faces round each vertex and a tree.
struct Spots<'a> {
    mesh: &'a Mesh,
    origin: &'a [u32],
    start: Vec<u32>,
    around: Vec<u32>,
    tree: Bvh,
}

impl Spots<'_> {
    fn tris(&self) -> &[[u32; 3]] {
        self.mesh.indices.as_chunks::<3>().0
    }

    fn ring(&self, v: u32) -> &[u32] {
        &self.around[self.start[v as usize] as usize..self.start[v as usize + 1] as usize]
    }

    /// Adds the faces sharing a vertex with the region's to it.
    fn grow(&self, region: &mut Vec<u32>, in_region: &mut [bool]) {
        let tris = self.tris();
        let mut more = Vec::new();
        for &t in region.iter() {
            for v in tris[t as usize] {
                for &u in self.ring(v) {
                    if !in_region[u as usize] {
                        in_region[u as usize] = true;
                        more.push(u);
                    }
                }
            }
        }
        region.extend(more);
    }

    /// The region's corners at positions where it has two corners or more, one of them on its
    /// border (sorted): bodies that touch there could not be told apart when the region is put
    /// back.
    fn pinched(&self, region: &[u32], in_region: &[bool]) -> Vec<u32> {
        let tris = self.tris();
        let mut at: std::collections::HashMap<[u32; 3], Vec<u32>> = Default::default();
        for &t in region {
            for v in tris[t as usize] {
                let list = at.entry(self.mesh.vertex(v).map(f32::to_bits)).or_default();
                if !list.contains(&v) {
                    list.push(v);
                }
            }
        }
        let on_border = |v: u32| self.ring(v).iter().any(|&u| !in_region[u as usize]);
        let mut out: Vec<u32> = at
            .into_values()
            .filter(|list| list.len() > 1 && list.iter().any(|&v| on_border(v)))
            .flatten()
            .collect();
        out.sort_unstable();
        out
    }
}

/// Wraps the spots of `mesh` (closed and outward, its faces from the input triangles `origin`)
/// again, each on its own (`rewrap_region`): the regions round them (the spots' faces, `rings`
/// rings of faces round them and every face whose box meets theirs, in connected pieces) are cut
/// out, wrapped against the whole surface, rounded to the grid (to f32 when not `on_grid`), and put
/// back where that crosses less. Outside the regions nothing changes, so only their faces need
/// checking. A piece that gets nowhere goes into `failed` and is not tried again the same way.
/// None when no region got better.
fn rewrap_spots(
    mesh: &Mesh,
    origin: &[u32],
    pairs: &[(u32, u32)],
    rings: usize,
    on_grid: bool,
    failed: &mut std::collections::HashSet<u64>,
    control: &mut Control,
) -> RepairResult<Option<Wrapped>> {
    control.check()?;
    let tris = mesh.indices.as_chunks::<3>().0;
    let n = tris.len();
    let (start, around) = vertex_rings(mesh.vertex_count(), tris);
    let spots = Spots {
        mesh,
        origin,
        start,
        around,
        tree: Bvh::new(&bvh::triangle_boxes(mesh)),
    };
    // The regions round all spots.
    let mut in_region = vec![false; n];
    let mut region: Vec<u32> = pairs.iter().flat_map(|&(a, b)| [a, b]).collect();
    region.sort_unstable();
    region.dedup();
    for &t in &region {
        in_region[t as usize] = true;
    }
    for _ in 0..rings {
        spots.grow(&mut region, &mut in_region);
    }
    let mut more = Vec::new();
    for &t in &region {
        spots.tree.query(&box_of(&exact::corners(mesh, t)), |u| {
            if !in_region[u as usize] {
                in_region[u as usize] = true;
                more.push(u);
            }
        });
    }
    region.extend(more);
    // Where bodies touch at a corner on the border, the region takes in the faces round every
    // copy of that corner, along the line they touch on.
    for _ in 0..PINCH_ROUNDS {
        let pinched = spots.pinched(&region, &in_region);
        if pinched.is_empty() {
            break;
        }
        for v in pinched {
            for &u in spots.ring(v) {
                if !in_region[u as usize] {
                    in_region[u as usize] = true;
                    region.push(u);
                }
            }
        }
    }
    // In connected pieces (through shared corners and crossing pairs), each wrapped on its own.
    region.sort_unstable();
    let index = |t: u32| region.binary_search(&t).unwrap_or(0) as u32;
    let mut parent: Vec<u32> = (0..region.len() as u32).collect();
    let mut first: std::collections::HashMap<u32, u32> = Default::default();
    for (i, &t) in region.iter().enumerate() {
        for v in tris[t as usize] {
            let j = *first.entry(v).or_insert(i as u32);
            join(&mut parent, i as u32, j);
        }
    }
    for &(a, b) in pairs {
        join(&mut parent, index(a), index(b));
    }
    let mut pieces: Vec<(u32, u32)> = (0..region.len() as u32)
        .map(|i| (root(&mut parent, i), region[i as usize]))
        .collect();
    pieces.sort_unstable();
    let pieces: Vec<Vec<u32>> = pieces
        .chunk_by(|x, y| x.0 == y.0)
        .map(|g| g.iter().map(|x| x.1).collect())
        .collect();
    let cancel = control.cancel_flag();
    let tried = &*failed;
    let done: Vec<RepairResult<(Option<Improved>, Option<u64>)>> = pieces
        .par_iter()
        .map(|piece| {
            let mut quiet = Control::new(cancel, |_| {});
            let mut mine = vec![false; n];
            for &t in piece {
                mine[t as usize] = true;
            }
            let before = pairs.iter().filter(|&&(a, _)| mine[a as usize]).count();
            // A piece is known by where its spots are: one that got nowhere is not tried again
            // the same way.
            let key = {
                use std::hash::{Hash, Hasher};
                let mut corners: Vec<[u32; 3]> = pairs
                    .iter()
                    .filter(|&&(a, _)| mine[a as usize])
                    .flat_map(|&(a, b)| [a, b])
                    .flat_map(|t| tris[t as usize].map(|v| mesh.vertex(v).map(f32::to_bits)))
                    .collect();
                corners.sort_unstable();
                corners.dedup();
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (corners, rings, on_grid).hash(&mut h);
                h.finish()
            };
            // A piece without crossings (taken in for its boxes) can only stay as it is.
            if tried.contains(&key) || before == 0 {
                return Ok((None, None));
            }
            if !spots.pinched(piece, &mine).is_empty() {
                return Ok((None, Some(key)));
            }
            // Near contacts snapped first, as the whole pass does; when that splits the region's
            // border or does not help, as it is.
            let mut best: Option<Local> = None;
            for snap in [true, false] {
                if best.as_ref().is_some_and(|local| local.crossings == 0) {
                    break;
                }
                if let Some(local) = rewrap_region(&spots, piece, &mine, snap, on_grid, &mut quiet)?
                    && local.crossings < best.as_ref().map_or(before, |l| l.crossings)
                {
                    best = Some(local);
                }
            }
            Ok(match best {
                Some(local) => (Some((piece.clone(), local)), None),
                None => (None, Some(key)),
            })
        })
        .collect();
    control.check()?;
    let mut better: Vec<(Vec<u32>, Local)> = Vec::new();
    for d in done {
        let (improved, nowhere) = d?;
        better.extend(improved);
        failed.extend(nowhere);
    }
    if better.is_empty() {
        return Ok(None);
    }
    // Put back: the faces outside the improved regions, then theirs.
    let mut gone = vec![false; n];
    for (piece, _) in &better {
        for &t in piece {
            gone[t as usize] = true;
        }
    }
    let mut kept_id = vec![u32::MAX; n];
    let mut indices = Vec::with_capacity(mesh.indices.len());
    let mut new_origin = Vec::with_capacity(n);
    for t in 0..n {
        if !gone[t] {
            kept_id[t] = new_origin.len() as u32;
            indices.extend_from_slice(&tris[t]);
            new_origin.push(origin[t]);
        }
    }
    let first_made = new_origin.len() as u32;
    let mut positions = mesh.positions.clone();
    for (_, local) in &better {
        let base = (positions.len() / 3) as u32;
        for p in &local.extra {
            positions.extend_from_slice(p);
        }
        for f in &local.faces {
            indices.extend(f.map(|v| if v >= FRESH { base + (v - FRESH) } else { v }));
        }
        new_origin.extend_from_slice(&local.origin);
    }
    let out = Mesh { positions, indices };
    // What still crosses: the pairs outside the improved regions as they were, and the new faces
    // against each other and the faces round them. Faces on edges that are not closed are found
    // again on the whole when there were any: a region fits back along its cut, so an edge inside
    // it that was not closed comes back open, and its (t, t) went with the region's faces. A
    // region only fits back when its open sides are the sides of the cut, so where everything was
    // closed it stays closed.
    let mut new_pairs: Vec<(u32, u32)> = pairs
        .iter()
        .filter(|&&(a, b)| a != b && !gone[a as usize] && !gone[b as usize])
        .map(|&(a, b)| (kept_id[a as usize], kept_id[b as usize]))
        .collect();
    let made = (out.triangle_count() as u32 - first_made) as usize;
    let corners = |i: usize| exact::corners(&out, first_made + i as u32);
    new_pairs.extend(
        (0..made)
            .into_par_iter()
            .flat_map_iter(|i| {
                let p = corners(i);
                let mut found = Vec::new();
                spots.tree.query(&box_of(&p), |u| {
                    if !gone[u as usize] && tritri::intersect(&p, &exact::corners(mesh, u)) {
                        found.push((kept_id[u as usize], first_made + i as u32));
                    }
                });
                found
            })
            .collect::<Vec<_>>(),
    );
    let made_tree = Bvh::new(&(0..made).map(|i| box_of(&corners(i))).collect::<Vec<_>>());
    new_pairs.extend(
        made_tree
            .self_pairs_parallel(|i, j| {
                tritri::intersect(&corners(i as usize), &corners(j as usize))
            })
            .into_iter()
            .map(|(i, j)| (first_made + i, first_made + j)),
    );
    if pairs.iter().any(|&(a, b)| a == b) {
        new_pairs.extend(unclosed_faces(&out).into_iter().map(|t| (t, t)));
    }
    new_pairs.sort_unstable();
    new_pairs.dedup();
    if new_pairs.len() >= pairs.len() {
        return Ok(None);
    }
    let check = Check {
        intersecting: new_pairs.iter().filter(|&&(a, b)| a != b).count(),
        ..Check::default()
    };
    Ok(Some((out, new_origin, check, new_pairs)))
}

/// One region of `spots` (faces `region`, marked in `in_region`) wrapped again on its own: cut out
/// as a mesh, near contacts snapped when `snap`, its arrangement built and classified against the
/// whole surface (`Outside`), its outer surface rounded to the grid as in `surround` with the
/// region's border kept where it is (or its new points rounded to f32 when not `on_grid`), and
/// mapped back onto the surface's vertices. None when it does not fit back exactly: its open sides
/// must be the sides of the cut, each made once.
fn rewrap_region(
    spots: &Spots,
    region: &[u32],
    in_region: &[bool],
    snap: bool,
    on_grid: bool,
    control: &mut Control,
) -> RepairResult<Option<Local>> {
    let mesh = spots.mesh;
    let tris = spots.tris();
    // The region as a mesh of its own.
    let mut local_of: std::collections::HashMap<u32, u32> = Default::default();
    let mut global_of: Vec<u32> = Vec::new();
    let mut positions: Vec<f32> = Vec::new();
    let mut indices: Vec<u32> = Vec::with_capacity(region.len() * 3);
    for &t in region {
        for v in tris[t as usize] {
            let l = *local_of.entry(v).or_insert_with(|| {
                global_of.push(v);
                positions.extend_from_slice(&mesh.vertex(v));
                (global_of.len() - 1) as u32
            });
            indices.push(l);
        }
    }
    let sub = Mesh { positions, indices };
    let (work, work_origin) = arrangement_input(&sub);
    let snapped = if snap {
        snap_contacts(&work, control)?
    } else {
        None
    };
    let (work, work_origin) = match snapped {
        Some((snapped, map)) => {
            let (clean, more) = arrangement_input(&snapped);
            (clean, follow(&work_origin, &follow(&map, &more)))
        }
        None => (work, work_origin),
    };
    let Ok(arr) = arrangement::build(&work, control)? else {
        return Ok(None);
    };
    let outside = Outside {
        mesh,
        tree: &spots.tree,
        skip: in_region,
    };
    let decision = classify(&arr, Some(outside), control)?;
    let surf = surface(&arr, &decision).without_cavities();
    let vertex_count = arr.vertex_count;
    drop(arr);
    // Its vertices on the surface: the region's own corners where they were (every copy of a
    // corner on the border is that corner: its fans meet outside the region), the others new.
    // Corners on the border stay where they are, on the grid already.
    let border: Vec<bool> = global_of
        .iter()
        .map(|&v| spots.ring(v).iter().any(|&u| !in_region[u as usize]))
        .collect();
    let on_border = |s: u32| {
        let id = surf.ids[s as usize] as usize;
        id < vertex_count && border[id]
    };
    let (zipped, round_origin, round_vertex) = if on_grid {
        round_surface(&surf, &on_border, control)?
    } else {
        in_f32(&surf)
    };
    let ztris = zipped.indices.as_chunks::<3>().0;
    let keep = without_dust(ztris, &|v| zipped.point(v), zipped.vertex_count())
        .unwrap_or_else(|| vec![true; ztris.len()]);
    let mut global = vec![u32::MAX; zipped.vertex_count()];
    let mut taken = vec![false; global_of.len()];
    let mut extra: Vec<[f32; 3]> = Vec::new();
    for (t, tri) in ztris.iter().enumerate() {
        if !keep[t] {
            continue;
        }
        for &v in tri {
            if global[v as usize] != u32::MAX {
                continue;
            }
            let id = surf.ids[round_vertex[v as usize] as usize] as usize;
            if id < vertex_count && (border[id] || !taken[id]) {
                taken[id] = true;
                global[v as usize] = global_of[id];
            } else {
                global[v as usize] = FRESH + extra.len() as u32;
                extra.push(zipped.vertex(v));
            }
        }
    }
    let made: Vec<[u32; 3]> = ztris
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(t, _)| t.map(|v| global[v as usize]))
        .collect();
    // It must fit: its open sides exactly the sides of the cut, each side made once, no face
    // without three corners.
    if made.iter().any(|&t| !distinct(t)) {
        return Ok(None);
    }
    let mut sides: Vec<(u32, u32)> = made
        .iter()
        .flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])])
        .collect();
    sides.sort_unstable();
    if sides.windows(2).any(|w| w[0] == w[1]) {
        return Ok(None);
    }
    let mut open: Vec<(u32, u32)> = sides
        .iter()
        .copied()
        .filter(|&(a, b)| sides.binary_search(&(b, a)).is_err())
        .collect();
    let mut cut: Vec<(u32, u32)> = region
        .iter()
        .flat_map(|&t| {
            let x = tris[t as usize];
            [(x[0], x[1]), (x[1], x[2]), (x[2], x[0])]
        })
        .filter(|&(a, b)| {
            spots.ring(a).iter().any(|&u| {
                !in_region[u as usize] && {
                    let y = tris[u as usize];
                    (0..3).any(|j| y[j] == b && y[(j + 1) % 3] == a)
                }
            })
        })
        .collect();
    open.sort_unstable();
    cut.sort_unstable();
    if open != cut {
        return Ok(None);
    }
    // Nor may an edge it makes between two of the surface's vertices, other than a side of the cut,
    // be an edge of a face round it: put back, that edge would have more than two sides.
    let outside = |a: u32, b: u32| {
        spots
            .ring(a)
            .iter()
            .any(|&u| !in_region[u as usize] && tris[u as usize].contains(&b))
    };
    if sides.iter().any(|&(a, b)| {
        a < FRESH && b < FRESH && cut.binary_search(&(a, b)).is_err() && outside(a, b)
    }) {
        return Ok(None);
    }
    let origin: Vec<u32> = (0..ztris.len())
        .filter(|&t| keep[t])
        .map(|t| {
            let piece =
                region[work_origin[surf.source[round_origin[t] as usize] as usize] as usize];
            spots.origin[piece as usize]
        })
        .collect();
    // What it crosses: itself, or the faces round it.
    let position = |v: u32| {
        if v >= FRESH {
            extra[(v - FRESH) as usize].map(f64::from)
        } else {
            mesh.point(v)
        }
    };
    let corners: Vec<[P3; 3]> = made.iter().map(|t| t.map(position)).collect();
    let mut crossings = 0;
    for p in &corners {
        spots.tree.query(&box_of(p), |u| {
            if !in_region[u as usize] && tritri::intersect(p, &exact::corners(mesh, u)) {
                crossings += 1;
            }
        });
    }
    let tree = Bvh::new(&corners.iter().map(|p| box_of(p)).collect::<Vec<_>>());
    crossings += tree
        .self_pairs_parallel(|i, j| tritri::intersect(&corners[i as usize], &corners[j as usize]))
        .len();
    Ok(Some(Local {
        faces: made,
        extra,
        origin,
        crossings,
    }))
}

// ---------------------------------------------------------------------------------------------
// 7. Cleanup and validation

/// The last cleanup, which only removes whole shells: dust (`without_dust`: closed shells without
/// positive volume, sheets and crumbs that rounding left), and closed shells that ended up flat or
/// inside others. The mesh and, per triangle, the triangle of `mesh` it came from.
fn drop_dust(mesh: Mesh, control: &mut Control) -> RepairResult<(Mesh, Vec<u32>)> {
    let mut origin: Vec<u32> = (0..mesh.triangle_count() as u32).collect();
    let mut mesh = mesh;
    let tris = mesh.indices.as_chunks::<3>().0;
    if let Some(keep) = without_dust(tris, &|v| mesh.point(v), mesh.vertex_count()) {
        let (next, map) = topology::keep_triangles(&mesh, &keep);
        origin = follow(&origin, &map);
        mesh = next;
    }
    let keep =
        inner_or_flat_shells(&mesh, control)?.unwrap_or_else(|| vec![true; mesh.triangle_count()]);
    let (mesh, map) = topology::keep_triangles(&mesh, &keep);
    Ok((mesh, follow(&origin, &map)))
}

/// What is wrong with a wrapped mesh.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Check {
    /// Border and non-manifold edges (one for an empty mesh).
    open: usize,
    /// Edges whose two triangles run the same way.
    misoriented: usize,
    /// Shells (joined through edges) with no positive volume.
    inside_out: usize,
    /// Intersecting triangle pairs.
    intersecting: usize,
}

impl Check {
    /// Good enough to return, with warnings for what is left: closed, oriented and outward.
    fn acceptable(&self) -> bool {
        self.open == 0 && self.misoriented == 0 && self.inside_out == 0
    }

    fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.intersecting > 0 {
            out.push(format!(
                "{} remain.",
                count_of(
                    self.intersecting,
                    "small self-intersection",
                    "small self-intersections"
                )
            ));
        }
        out
    }
}

/// The full check: the shape and the intersection test over all faces.
#[cfg(test)]
fn check(mesh: &Mesh, control: &mut Control) -> RepairResult<Check> {
    Ok(Check {
        intersecting: tritri::intersecting_pairs(mesh, control)?.len(),
        ..shape_check(mesh)
    })
}

// ---------------------------------------------------------------------------------------------
// 8. Last resort

/// Cells of clearance between rebuilt and kept shells.
const REBUILD_MARGIN: f64 = 2.0;

/// Grids tried at most, each coarser when the last made far more triangles than aimed for.
const REBUILD_TRIES: usize = 2;

/// The last resort, for a result `out` (per triangle its input triangle, `origin`) whose `bad`
/// triangles are open, misoriented or crossing: the shells holding them, and every shell whose box
/// comes within `REBUILD_MARGIN` cells of theirs, rebuilt from the solid of `solid` (the prepared
/// part, closed and balanced) on a grid (`levelset`), closed, oriented and without crossings
/// whatever the input; the other shells kept as they are. Cavities and shells inside others go,
/// as in a wrapped result. With `bad` empty, all of `out` is rebuilt. The warning starts with
/// `why`. None when the solid holds nothing there to rebuild.
fn rebuilt(
    input: &Mesh,
    out: &Mesh,
    origin: &[u32],
    bad: &[u32],
    solid: &Mesh,
    why: &str,
    control: &mut Control,
) -> RepairResult<Option<Outcome>> {
    control.stage("Rebuilding the surface on a grid");
    let (shell_of, count) = topology::components(out, &Edges::new(out), Link::Vertex);
    let mut boxes = vec![bvh::EMPTY; count];
    for (t, tri) in out.indices.as_chunks::<3>().0.iter().enumerate() {
        for &v in tri {
            bvh::include(&mut boxes[shell_of[t] as usize], exact::vertex(out, v));
        }
    }
    let mut rebuild = vec![bad.is_empty(); count];
    for &t in bad {
        rebuild[shell_of[t as usize] as usize] = true;
    }
    let mut region = bvh::EMPTY;
    if bad.is_empty() {
        solid_bounds(solid, &mut region);
    }
    for s in 0..count {
        if rebuild[s] {
            bvh::grow(&mut region, &boxes[s]);
        }
    }
    let faces = |rebuild: &[bool]| {
        (0..out.triangle_count() as u32)
            .filter(|&t| rebuild[shell_of[t as usize] as usize])
            .collect::<Vec<u32>>()
    };
    let first = faces(&rebuild);
    let budget = levelset::target_triangles(first.len());
    let mut cell = levelset::cell_for(levelset::area_of(out, first.into_iter()), budget, &region);
    let mut tries = 0;
    let remeshed = loop {
        tries += 1;
        let margin = REBUILD_MARGIN * cell;
        // Shells near those rebuilt are rebuilt with them, until none is near.
        let mut zones: Vec<Aabb>;
        loop {
            zones = (0..count)
                .filter(|&s| rebuild[s])
                .map(|s| grown(boxes[s], margin))
                .collect();
            if bad.is_empty() {
                let mut all = bvh::EMPTY;
                solid_bounds(solid, &mut all);
                zones.push(grown(all, margin));
            }
            let near: Vec<usize> = (0..count)
                .filter(|&s| !rebuild[s] && zones.iter().any(|g| bvh::overlap(g, &boxes[s])))
                .collect();
            if near.is_empty() {
                break;
            }
            for s in near {
                rebuild[s] = true;
            }
        }
        // The solid inside the shells' boxes, a cell round them: the rebuilt surface stays
        // within a cell of that, clear of the kept shells.
        let within: Vec<Aabb> = zones.iter().map(|&g| grown(g, cell - margin)).collect();
        let Some(remeshed) =
            levelset::remesh(solid, Some(&within), cell, &mut control.sub(0.0, 0.9))?
        else {
            return Ok(None);
        };
        // Walls thinner than a cell take more triangles than their area suggests: far over the
        // budget, once more on a coarser grid.
        let made = remeshed.mesh.triangle_count() as f64;
        if tries == REBUILD_TRIES || made <= 1.25 * budget as f64 {
            break remeshed;
        }
        cell *= (made / budget as f64).sqrt();
    };
    let fresh = match inner_or_flat_shells(&remeshed.mesh, &mut control.sub(0.9, 1.0))? {
        Some(keep) => topology::keep_triangles(&remeshed.mesh, &keep).0,
        None => remeshed.mesh,
    };
    let keep: Vec<bool> = shell_of.iter().map(|&s| !rebuild[s as usize]).collect();
    let (kept, map) = topology::keep_triangles(out, &keep);
    if fresh.indices.is_empty() && kept.indices.is_empty() {
        return Ok(None);
    }
    let mut origin = follow(origin, &map);
    origin.resize(kept.triangle_count() + fresh.triangle_count(), NEW);
    let joined = crate::ops::concat(&[&kept, &fresh]).map_err(super::RepairError::Failed)?;
    let joined = crate::ops::compact(&joined.positions, joined.indices);
    let shells = rebuild.iter().filter(|&&r| r).count();
    let cell_text = format!("{:.3}", remeshed.cell)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string();
    let what = if keep.iter().all(|&k| !k) {
        "the whole surface was".to_string()
    } else if shells == 1 {
        "1 shell was".to_string()
    } else {
        format!("{} were", count_of(shells, "shell", "shells"))
    };
    let warning = format!(
        "{why}, so {what} rebuilt on a {cell_text} mm grid: detail below {cell_text} mm may be \
         softened."
    );
    Ok(Some(finish(input, joined, origin, vec![warning])))
}

/// Grows `b` to hold every vertex of `mesh`.
fn solid_bounds(mesh: &Mesh, b: &mut Aabb) {
    for v in 0..mesh.vertex_count() as u32 {
        bvh::include(b, exact::vertex(mesh, v));
    }
}

/// `check` without the intersection test.
fn shape_check(mesh: &Mesh) -> Check {
    let edges = Edges::new(mesh);
    let mut check = Check {
        open: mesh.indices.is_empty() as usize,
        ..Check::default()
    };
    for e in 0..edges.len() as u32 {
        if edges.degree(e) != 2 {
            check.open += 1;
        } else if !topology::is_consistent(mesh, &edges, e) {
            check.misoriented += 1;
        }
    }
    let (shell_of, shells) = topology::components(mesh, &edges, Link::Edge);
    drop(edges);
    let tris = mesh.indices.as_chunks::<3>().0;
    let mut volume = vec![0.0f64; shells];
    let mut apex: Vec<Option<P3>> = vec![None; shells];
    for (t, tri) in tris.iter().enumerate() {
        let s = shell_of[t] as usize;
        let o = *apex[s].get_or_insert(mesh.point(tri[0]));
        volume[s] += volume6(o, tri.map(|v| mesh.point(v)));
    }
    check.inside_out = volume.iter().filter(|&&v| v <= 0.0).count();
    check
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{make_box, stats};
    use crate::ops::concat;
    use crate::repair::silent;
    use crate::repair::winding::Solid;
    use std::time::Instant;

    fn wrap(mesh: &Mesh) -> Outcome {
        silent(|c| run(mesh, &[], c)).expect("wrap runs")
    }

    /// Closed (every edge two opposite sides), positive volume per edge-joined shell, and no
    /// intersecting triangles.
    fn assert_printable(mesh: &Mesh) {
        let c = silent(|control| check(mesh, control)).unwrap();
        assert_eq!(c, Check::default(), "problems left: {c:?}");
    }

    /// A box with half-sizes `half`, rotated by the unit quaternion `q`, centred at `center`.
    fn rotated_box(center: P3, half: P3, q: [f64; 4]) -> Mesh {
        let mut m = make_box([-1.0; 3], [1.0; 3]);
        for p in m.positions.as_chunks_mut::<3>().0 {
            let local = [
                p[0] as f64 * half[0],
                p[1] as f64 * half[1],
                p[2] as f64 * half[2],
            ];
            let r = rotate(q, local);
            for k in 0..3 {
                p[k] = (r[k] + center[k]) as f32;
            }
        }
        m
    }

    fn rotate(q: [f64; 4], v: P3) -> P3 {
        let [w, x, y, z] = q;
        let u = [x, y, z];
        let t = exact::scale(exact::cross(u, v), 2.0);
        exact::add(exact::add(v, exact::scale(t, w)), exact::cross(u, t))
    }

    /// A deterministic pseudo-random generator for the fixtures.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }

        fn quaternion(&mut self) -> [f64; 4] {
            let q = [
                self.next() - 0.5,
                self.next() - 0.5,
                self.next() - 0.5,
                self.next() - 0.5,
            ];
            let n = q.iter().map(|x| x * x).sum::<f64>().sqrt();
            q.map(|x| x / n)
        }
    }

    /// A closed UV sphere with outward-facing triangles: `stacks` rings, `slices` segments.
    fn sphere(center: P3, radius: f64, stacks: usize, slices: usize) -> Mesh {
        let mut positions = Vec::new();
        let mut push = |p: P3| {
            positions
                .extend([p[0] + center[0], p[1] + center[1], p[2] + center[2]].map(|x| x as f32))
        };
        push([0.0, 0.0, radius]);
        for i in 1..stacks {
            let theta = std::f64::consts::PI * i as f64 / stacks as f64;
            for j in 0..slices {
                let phi = 2.0 * std::f64::consts::PI * j as f64 / slices as f64;
                push([
                    radius * theta.sin() * phi.cos(),
                    radius * theta.sin() * phi.sin(),
                    radius * theta.cos(),
                ]);
            }
        }
        push([0.0, 0.0, -radius]);
        let ring = |i: usize, j: usize| (1 + (i - 1) * slices + j % slices) as u32;
        let bottom = (1 + (stacks - 1) * slices) as u32;
        let mut indices = Vec::new();
        for j in 0..slices {
            indices.extend_from_slice(&[0, ring(1, j), ring(1, j + 1)]);
            indices.extend_from_slice(&[bottom, ring(stacks - 1, j + 1), ring(stacks - 1, j)]);
        }
        for i in 1..stacks - 1 {
            for j in 0..slices {
                let (a, b, c, d) = (
                    ring(i, j),
                    ring(i, j + 1),
                    ring(i + 1, j),
                    ring(i + 1, j + 1),
                );
                indices.extend_from_slice(&[a, c, d, a, d, b]);
            }
        }
        Mesh { positions, indices }
    }

    fn volume(mesh: &Mesh) -> f64 {
        stats(mesh).volume
    }

    /// The wrapped mesh bounds exactly the union of `bodies`: at `samples` pseudo-random points in
    /// the bounding box, its winding number is 1 inside some body and 0 elsewhere. No triangle is
    /// flat either.
    fn assert_union(out: &Mesh, bodies: &[Mesh], samples: usize, seed: u64) {
        let tris = out.indices.as_chunks::<3>().0;
        let flat = tris
            .iter()
            .filter(|t| exact::collinear(out.point(t[0]), out.point(t[1]), out.point(t[2])))
            .count();
        assert_eq!(flat, 0, "flat triangles left");
        let wrapped = Solid::from_mesh(out, None);
        let solids: Vec<Solid> = bodies.iter().map(|b| Solid::from_mesh(b, None)).collect();
        let [lo, hi] = crate::mesh::bounds(out).unwrap();
        let mut rng = Rng(seed);
        let mut inside = 0;
        for _ in 0..samples {
            let p: P3 = [0, 1, 2].map(|k| {
                let (a, b) = (lo[k] as f64 - 1.0, hi[k] as f64 + 1.0);
                a + (b - a) * rng.next()
            });
            let Some(w) = wrapped.winding(p) else {
                continue;
            };
            let union = solids.iter().any(|s| s.winding(p).is_some_and(|w| w != 0));
            assert_eq!(
                w, union as i32,
                "winding {w} at {p:?}, inside a body: {union}"
            );
            inside += union as usize;
        }
        assert!(inside > 0, "no sample landed inside");
    }

    #[test]
    fn a_single_cube_has_nothing_to_wrap() {
        let cube = make_box([0.0; 3], [20.0; 3]);
        let out = wrap(&cube);
        assert!(!out.changed);
        assert_eq!(out.mesh, cube);
        assert_eq!(out.message, "Nothing to wrap.");
    }

    #[test]
    fn two_interpenetrating_cubes_become_one_box() {
        let a = make_box([0.0; 3], [20.0; 3]);
        let b = make_box([10.0, 0.0, 0.0], [30.0, 20.0, 20.0]);
        let out = wrap(&concat(&[&a, &b]).unwrap());
        assert!(out.changed);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        let s = stats(&out.mesh);
        assert_eq!(s.shells, 1);
        assert!(
            (s.volume - 12000.0).abs() <= 12000.0 * 1e-6,
            "volume {}",
            s.volume
        );
        assert_printable(&out.mesh);
        assert_union(&out.mesh, &[a.clone(), b.clone()], 4000, 1);
        for p in out.mesh.positions.as_chunks::<3>().0 {
            assert!(
                p[0] == 0.0
                    || p[0] == 30.0
                    || p[1] == 0.0
                    || p[1] == 20.0
                    || p[2] == 0.0
                    || p[2] == 20.0,
                "vertex {p:?} is off the box"
            );
        }
        let Origin::Map(origin) = &out.origin else {
            panic!("an origin map");
        };
        assert_eq!(origin.len(), out.mesh.triangle_count());
        assert!(origin.iter().all(|&t| t < 24));
        // Wrapping the result again changes nothing.
        assert!(!wrap(&out.mesh).changed);
    }

    #[test]
    fn a_cube_inside_a_cube_is_removed() {
        let outer = make_box([0.0; 3], [20.0; 3]);
        let inner = make_box([5.0; 3], [10.0; 3]);
        let out = wrap(&concat(&[&outer, &inner]).unwrap());
        assert!(out.changed);
        assert_eq!(out.mesh, outer);
        assert_eq!(
            out.message,
            "Wrapped the surface: removed 12 inner triangles, 1 shell left."
        );
    }

    #[test]
    fn a_cavity_is_filled() {
        let outer = make_box([0.0; 3], [20.0; 3]);
        let mut inner = make_box([5.0; 3], [10.0; 3]);
        for t in inner.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
        let out = wrap(&concat(&[&outer, &inner]).unwrap());
        assert_eq!(out.mesh, outer);
    }

    #[test]
    fn three_rotated_boxes_with_triple_points() {
        let mut rng = Rng(7);
        let boxes: Vec<Mesh> = (0..3)
            .map(|k| {
                let c = [k as f64 * 6.0, (k % 2) as f64 * 5.0, 0.0];
                rotated_box(c, [10.0, 7.0, 6.0], rng.quaternion())
            })
            .collect();
        let refs: Vec<&Mesh> = boxes.iter().collect();
        let input = concat(&refs).unwrap();
        let out = wrap(&input);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert_printable(&out.mesh);
        assert_eq!(stats(&out.mesh).shells, 1);
        assert_union(&out.mesh, &boxes, 4000, 2);
        let largest = boxes.iter().map(volume).fold(0.0, f64::max);
        let sum: f64 = boxes.iter().map(volume).sum();
        let v = volume(&out.mesh);
        assert!(
            v >= largest - 1e-3 && v <= sum + 1e-3,
            "{v} not in {largest}..{sum}"
        );
        assert!(!wrap(&out.mesh).changed);
    }

    #[test]
    fn a_shell_that_passes_through_itself() {
        // Two boxes joined into one shell by a shared corner vertex, their bodies crossing: box b
        // (12 mm, tilted) hangs from a's corner (20, 20, 20), partly inside a and partly out.
        let a = make_box([0.0; 3], [20.0; 3]);
        let axis = [0.3f64, -0.5, 0.81];
        let (n, half) = (exact::length(axis), 0.4f64);
        let q = [
            half.cos(),
            axis[0] / n * half.sin(),
            axis[1] / n * half.sin(),
            axis[2] / n * half.sin(),
        ];
        let mut b = rotated_box([0.0; 3], [6.0; 3], q);
        let corner = b.vertex(6);
        for p in b.positions.as_chunks_mut::<3>().0 {
            for k in 0..3 {
                p[k] = p[k] - corner[k] + 20.0;
            }
        }
        let mut joined = concat(&[&a, &b]).unwrap();
        for i in joined.indices.iter_mut() {
            if *i == 8 + 6 {
                *i = 6;
            }
        }
        let (_, touching) = topology::components(&joined, &Edges::new(&joined), Link::Vertex);
        assert_eq!(touching, 1);
        assert!(
            !silent(|c| tritri::intersecting_pairs(&joined, c))
                .unwrap()
                .is_empty()
        );
        let out = wrap(&joined);
        assert!(out.changed);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert_printable(&out.mesh);
        assert_union(&out.mesh, &[a.clone(), b.clone()], 4000, 3);
        let v = volume(&out.mesh);
        assert!(v > 8000.0 && v < 8000.0 + 1728.0, "volume {v}");
        assert!(!wrap(&out.mesh).changed);
    }

    #[test]
    fn random_overlapping_boxes_and_spheres() {
        let mut rng = Rng(42);
        let mut bodies: Vec<Mesh> = Vec::new();
        for _ in 0..6 {
            let c = [rng.next() * 20.0, rng.next() * 20.0, rng.next() * 20.0];
            let h = [
                3.0 + rng.next() * 6.0,
                3.0 + rng.next() * 6.0,
                3.0 + rng.next() * 6.0,
            ];
            bodies.push(rotated_box(c, h, rng.quaternion()));
        }
        for _ in 0..3 {
            let c = [rng.next() * 20.0, rng.next() * 20.0, rng.next() * 20.0];
            bodies.push(sphere(c, 4.0 + rng.next() * 4.0, 12, 18));
        }
        let refs: Vec<&Mesh> = bodies.iter().collect();
        let input = concat(&refs).unwrap();
        let out = wrap(&input);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert_printable(&out.mesh);
        assert_union(&out.mesh, &bodies, 4000, 4);
        let largest = bodies.iter().map(volume).fold(0.0, f64::max);
        let sum: f64 = bodies.iter().map(volume).sum();
        let v = volume(&out.mesh);
        assert!(
            v >= largest - 1e-3 && v <= sum + 1e-3,
            "{v} not in {largest}..{sum}"
        );
        // The same input gives the same output, and a second run changes nothing.
        assert_eq!(wrap(&input).mesh, out.mesh);
        assert!(!wrap(&out.mesh).changed);
    }

    #[test]
    fn boxes_touching_along_an_edge_stay_two_shells() {
        // Two cubes that share only an edge line, plus a third crossing both so there is something
        // to wrap: the union touches itself along that edge, which must not become non-manifold.
        let a = make_box([0.0; 3], [10.0; 3]);
        let b = make_box([10.0, 10.0, 0.0], [20.0, 20.0, 10.0]);
        let c = make_box([2.0, 2.0, 2.0], [5.0, 5.0, 12.0]);
        let out = wrap(&concat(&[&a, &b, &c]).unwrap());
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert_printable(&out.mesh);
        assert_eq!(stats(&out.mesh).shells, 2);
        let v = volume(&out.mesh);
        assert!((v - (2000.0 + 18.0)).abs() < 1e-3, "volume {v}");
    }

    #[test]
    fn a_part_resting_on_a_slightly_tilted_face_joins_the_body() {
        // The body's +x face leans out by 0.0000875 mm across its width (vertices 2 and 6, at
        // y = 10, sit that much farther out), as a CAD export left it; the part's flat face lies
        // at that outermost x, so between them is a gap of up to 0.00007 mm and no exact contact
        // (as on a real CAD export). A third box crosses the body so there is something to wrap. Closer
        // than SNAP_APART, the part joins the body, as in the reference tool.
        let lean = 10.000_088;
        let mut body = make_box([0.0; 3], [10.0; 3]);
        for v in [2, 6] {
            body.positions[v * 3] = lean;
        }
        let part = make_box([lean, 2.0, 2.0], [12.0, 8.0, 8.0]);
        let pin = make_box([2.0, 2.0, 2.0], [5.0, 5.0, 12.0]);
        let out = wrap(&concat(&[&body, &part, &pin]).unwrap());
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert_printable(&out.mesh);
        assert_eq!(stats(&out.mesh).shells, 1);
        let v = volume(&out.mesh);
        assert!((v - (1000.0 + 72.0 + 18.0)).abs() < 0.01, "volume {v}");
        assert!(!wrap(&out.mesh).changed);
    }

    #[test]
    fn boxes_stacked_face_to_face_fuse() {
        // Stacked with partly overlapping faces, and one more crossing them: the touching faces
        // cancel where they overlap, the side faces join along the seam.
        let a = make_box([0.0; 3], [10.0, 10.0, 5.0]);
        let b = make_box([4.0, 2.0, 5.0], [14.0, 8.0, 9.0]);
        let c = make_box([1.0, 1.0, 1.0], [3.0, 3.0, 7.0]);
        let parts = [a, b, c];
        let refs: Vec<&Mesh> = parts.iter().collect();
        let out = wrap(&concat(&refs).unwrap());
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert_printable(&out.mesh);
        assert_eq!(stats(&out.mesh).shells, 1);
        let v = volume(&out.mesh);
        assert!((v - (500.0 + 240.0 + 8.0)).abs() < 1e-3, "volume {v}");
        assert_union(&out.mesh, &parts, 3000, 5);
    }

    /// Every edge has as many sides running one way as the other.
    fn balanced(mesh: &Mesh) -> bool {
        let edges = Edges::new(mesh);
        (0..edges.len() as u32).all(|e| {
            let [lo, _] = edges.ends(e);
            let up = edges
                .sides(e)
                .iter()
                .filter(|&&s| topology::side_ends(mesh, s).0 == lo)
                .count();
            up * 2 == edges.degree(e)
        })
    }

    #[test]
    fn a_large_face_crossed_by_many_small_bodies_near_its_edges() {
        // Thin pins through the top of a slab, many of them across its edges, so the slab's top
        // triangles are units of one triangle whose crossing segments start on their sides.
        let mut parts = vec![make_box([0.0; 3], [20.0, 20.0, 2.0])];
        let mut rng = Rng(3);
        for i in 0..40 {
            let (x, y) = if i % 2 == 0 {
                (rng.next() * 20.0, 20.0 * (i % 4 / 2) as f64)
            } else {
                (20.0 * (i % 4 / 2) as f64, rng.next() * 20.0)
            };
            let q = rng.quaternion();
            parts.push(rotated_box([x, y, 2.0], [0.3, 0.2, 1.0], q));
        }
        let refs: Vec<&Mesh> = parts.iter().collect();
        let out = wrap(&concat(&refs).unwrap());
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert_printable(&out.mesh);
        assert_union(&out.mesh, &parts, 4000, 9);
    }

    #[test]
    fn ear_clipping_triangulates_simple_polygons() {
        // Star-shaped polygons with many corners, both ways round: n − 2 triangles, all turning
        // the polygon's way, covering exactly its area.
        let mut rng = Rng(17);
        for case in 0..60 {
            let n = 3 + (rng.next() * 300.0) as usize;
            let mut flat: Vec<[f64; 2]> = (0..n)
                .map(|i| {
                    let a = std::f64::consts::TAU * (i as f64 + 0.5 * rng.next()) / n as f64;
                    let r = 1.0 + 4.0 * rng.next();
                    [r * a.cos(), r * a.sin()]
                })
                .collect();
            let turn = if case % 2 == 0 { 1 } else { -1 };
            if turn < 0 {
                flat.reverse();
            }
            let poly: Vec<u32> = (0..n as u32).collect();
            let tris = ear_clip(&poly, &flat, turn).expect("a simple polygon has ears");
            assert_eq!(tris.len(), n - 2);
            let area2 = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
                (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
            };
            let mut total = 0.0;
            for t in &tris {
                let [a, b, c] = t.map(|v| flat[v as usize]);
                assert_eq!(exact::orient2d(a, b, c), turn);
                total += area2(a, b, c).abs();
            }
            let polygon: f64 = (0..n)
                .map(|i| area2([0.0; 2], flat[i], flat[(i + 1) % n]))
                .sum::<f64>()
                .abs();
            assert!(
                (total - polygon).abs() < 1e-9 * polygon,
                "{total} vs {polygon}"
            );
        }
    }

    #[test]
    fn sheets_go_and_thin_slabs_stay() {
        // A square closed with a copy of itself facing the other way (an open sheet that Close
        // all holes closed), a hair off: no material. A slab a tenth of a unit thick: material.
        let mut w = Welder::new(0);
        let z = 1e-6f32;
        w.triangle([0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0]);
        w.triangle([0.0, 0.0, 0.0], [10.0, 10.0, 0.0], [0.0, 10.0, z]);
        w.triangle([0.0, 0.0, 0.0], [10.0, 10.0, 0.0], [10.0, 0.0, 0.0]);
        w.triangle([0.0, 0.0, 0.0], [0.0, 10.0, z], [10.0, 10.0, 0.0]);
        let sheet = w.finish().unwrap();
        let slab = make_box([0.0, 0.0, 5.0], [10.0, 10.0, 5.1]);
        let both = concat(&[&sheet, &slab]).unwrap();
        let (kept, _) = without_sheets(&both).expect("the sheet goes");
        assert_eq!(kept.triangle_count(), slab.triangle_count());
        assert!(without_sheets(&slab).is_none());
    }

    #[test]
    fn dust_and_crumbs_go_but_small_bodies_stay() {
        let big = make_box([0.0; 3], [10.0; 3]);
        // A crumb: a tiny box sharing a corner position with the big one.
        let crumb = make_box([10.0; 3], [10.01; 3]);
        // A small body on its own, well away.
        let small = make_box([20.0; 3], [20.5; 3]);
        // A cavity: a box turned inside out.
        let mut cavity = make_box([2.0; 3], [3.0; 3]);
        for t in cavity.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
        let all = concat(&[&big, &crumb, &small, &cavity]).unwrap();
        let tris = all.indices.as_chunks::<3>().0;
        let keep = without_dust(tris, &|v| all.point(v), all.vertex_count()).expect("dust goes");
        let n = big.triangle_count();
        assert!(keep[..n].iter().all(|&k| k), "the big box stays");
        assert!(keep[n..2 * n].iter().all(|&k| !k), "the crumb goes");
        assert!(keep[2 * n..3 * n].iter().all(|&k| k), "the small box stays");
        assert!(keep[3 * n..].iter().all(|&k| !k), "the cavity goes");
    }

    #[test]
    fn snapping_keeps_every_edge_balanced() {
        // Grid boxes whose faces lie within an f32 unit or two of each other's, at random: the
        // snap welds, splits edges and inserts corners, and must never open the surface.
        let mut rng = Rng(21);
        let mut snapped_any = false;
        for _ in 0..40 {
            let a = grid_box([0.0; 3], [8.0; 3], 4);
            let unit = ulp_f32(16.0) as f32;
            let jitter = |rng: &mut Rng| (rng.next() as f32 - 0.5) * 3.0 * unit;
            let lo = [
                8.0 + jitter(&mut rng),
                2.0 + rng.next() as f32,
                1.5 + rng.next() as f32,
            ];
            let hi = [14.0 + jitter(&mut rng), lo[1] + 4.0, lo[2] + 4.0];
            let b = grid_box(lo, hi, 3);
            let (input, _) = arrangement_input(&concat(&[&a, &b]).unwrap());
            if let Some((snapped, _)) = silent(|c| snap_contacts(&input, c)).unwrap() {
                snapped_any = true;
                let (clean, _) = arrangement_input(&snapped);
                assert!(balanced(&clean));
            }
        }
        assert!(snapped_any);
    }

    #[test]
    fn crossings_left_are_wrapped_again_locally() {
        // Two interpenetrating boxes, unwrapped: their crossing faces and the rings round them
        // are wrapped again on their own and put back, which leaves the union.
        let a = grid_box([0.0; 3], [10.0; 3], 3);
        let b = rotated_box([10.0, 5.0, 5.0], [3.0, 2.0, 2.0], [0.96, 0.1, 0.2, 0.15]);
        let both = concat(&[&a, &b]).unwrap();
        let pairs = silent(|c| tritri::intersecting_pairs(&both, c)).unwrap();
        assert!(!pairs.is_empty());
        let origin: Vec<u32> = (0..both.triangle_count() as u32).collect();
        let (out, out_origin, local, left) = silent(|c| {
            rewrap_spots(
                &both,
                &origin,
                &pairs,
                SPOT_RINGS,
                true,
                &mut Default::default(),
                c,
            )
        })
        .unwrap()
        .expect("the spots get better");
        assert_eq!(local.intersecting, left.len());
        assert_eq!(out_origin.len(), out.triangle_count());
        assert!(balanced(&out));
        let full = silent(|c| check(&out, c)).unwrap();
        assert_eq!(
            full.intersecting,
            left.len(),
            "the local count is the true one"
        );
        assert_eq!(full, Check::default(), "problems left: {full:?}");
        assert_union(&out, &[a, b], 3000, 13);
    }

    #[test]
    fn crossings_left_are_covered_with_blocks() {
        // Two boxes that overlap by 0.001 mm, as if the exact wrap had left the crossings round
        // that slab: one block over the slab joins both into one closed shell that crosses
        // nothing, and every face away from the block keeps its corners.
        let a = grid_box([0.0; 3], [10.0; 3], 3);
        let b = make_box([9.999, 4.0, 4.0], [20.0, 6.0, 6.0]);
        let both = concat(&[&a, &b]).unwrap();
        let pairs = silent(|c| tritri::intersecting_pairs(&both, c)).unwrap();
        let w: Wrapped = (
            both.clone(),
            (0..both.triangle_count() as u32).collect(),
            Check {
                intersecting: pairs.len(),
                ..shape_check(&both)
            },
            pairs,
        );
        let (out, size) = silent(|c| covered(&w, c, &mut Timings::default()))
            .unwrap()
            .expect("fewer crossings");
        assert_eq!(out.2.intersecting, 0);
        assert_printable(&out.0);
        let s = stats(&out.0);
        assert_eq!(s.shells, 1);
        // The union, plus the block's bit of slab and rim: about 0.02 mm across the slab.
        assert!(size > 2.0 && size < 2.1, "{size}");
        let union = 1000.0 + 10.001 * 4.0;
        assert!((s.volume - union).abs() < 0.02, "{} vs {union}", s.volume);
        assert_eq!(out.1.len(), out.0.triangle_count());
        // The far end of the bar is untouched.
        let far = out
            .0
            .positions
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|p| p[0] == 20.0)
            .count();
        assert_eq!(far, 4);
    }

    #[test]
    fn touching_cover_boxes_merge() {
        let mesh = concat(&[
            &make_box([0.0; 3], [1.0; 3]),
            &make_box([0.5, 0.0, 0.0], [1.5, 1.0, 1.0]),
            &make_box([5.0; 3], [6.0; 3]),
        ])
        .unwrap();
        // Faces 0, 1, 12 and 13 are the bottoms of the first two boxes: the boxes round both
        // pairs touch and merge. Faces 24 and 25, the third box's bottom, stay apart.
        let boxes = cover_boxes(&mesh, &[(0, 12), (1, 13), (24, 25)], 0.01);
        assert_eq!(boxes.len(), 2, "{boxes:?}");
        assert!(
            boxes
                .iter()
                .any(|b| b[0][0] < 0.5 && b[1][1] > 1.0 && b[1][2] < 0.1)
        );
    }

    #[test]
    fn long_faces_crossing_at_a_point_get_a_small_box() {
        // Two slivers 100 mm long whose boxes overlap over 100 mm, crossing within 0.1 mm.
        let p = [[0.0, 0.0, 0.0], [100.0, 100.0, 0.0], [100.0, 100.1, 0.0]];
        let q = [[50.0, 50.05, -1.0], [50.05, 50.0, 1.0], [0.0, 100.0, 0.0]];
        let b = crossing_box(p, q);
        assert!((0..3).all(|k| b[1][k] - b[0][k] < 1.0), "{b:?}");
        assert!((49.9..50.2).contains(&b[0][0]), "{b:?}");
    }

    #[test]
    fn rebuilding_keeps_the_shells_away_from_the_bad_ones() {
        let near = make_box([0.0; 3], [4.0; 3]);
        let far = make_box([20.0, 0.0, 0.0], [24.0, 4.0, 4.0]);
        let both = concat(&[&near, &far]).unwrap();
        let origin: Vec<u32> = (0..both.triangle_count() as u32).collect();
        let out = silent(|c| rebuilt(&both, &both, &origin, &[0], &both, "Test", c))
            .unwrap()
            .expect("rebuilt");
        assert_printable(&out.mesh);
        assert_eq!(stats(&out.mesh).shells, 2);
        // The far box keeps its own faces, the near one is new.
        let Origin::Map(map) = &out.origin else {
            panic!("no origin map")
        };
        let kept: Vec<u32> = map.iter().copied().filter(|&t| t != NEW).collect();
        assert_eq!(kept, (12..24).collect::<Vec<u32>>());
        assert!(
            out.warnings[0].contains("1 shell was rebuilt"),
            "{:?}",
            out.warnings
        );
    }

    /// A box subdivided into an n × n grid on every face (a different triangulation from
    /// make_box), outward-facing.
    fn grid_box(min: [f32; 3], max: [f32; 3], n: usize) -> Mesh {
        let mut w = Welder::new(0);
        let lerp = |k: usize, i: usize| min[k] + (max[k] - min[k]) * i as f32 / n as f32;
        // For each axis and side, a grid of quads facing out.
        for axis in 0..3 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for side in 0..2 {
                for i in 0..n {
                    for j in 0..n {
                        let corner = |di: usize, dj: usize| {
                            let mut p = [0.0f32; 3];
                            p[axis] = if side == 0 { min[axis] } else { max[axis] };
                            p[u] = lerp(u, i + di);
                            p[v] = lerp(v, j + dj);
                            p
                        };
                        let (a, b, c, d) = (corner(0, 0), corner(1, 0), corner(1, 1), corner(0, 1));
                        if side == 1 {
                            w.triangle(a, b, c);
                            w.triangle(a, c, d);
                        } else {
                            w.triangle(a, c, b);
                            w.triangle(a, d, c);
                        }
                    }
                }
            }
        }
        w.finish().unwrap()
    }

    /// Scenes of every awkward kind (coplanar faces, touching bodies, nearly coincident copies,
    /// cavities), each wrapped and checked: printable, bounding the union, and unchanged by a
    /// second run. `STRESS_SCENE=n` runs one scene with details.
    #[test]
    #[ignore]
    fn stress_scenes() {
        let mut failed = 0;
        let started = Instant::now();
        for scene in 0..800 {
            let kind = scene % 8;
            let mut rng = Rng(2026 + scene as u64 * 7919);
            let mut bodies: Vec<Mesh> = Vec::new();
            match kind {
                0 => {
                    // Axis-aligned boxes on an integer grid: coplanar faces, touching faces,
                    // shared edges and corners everywhere.
                    for _ in 0..5 {
                        let p: [f32; 3] = [0, 1, 2].map(|_| (rng.next() * 6.0).floor() as f32);
                        let s: [f32; 3] =
                            [0, 1, 2].map(|_| 1.0 + (rng.next() * 4.0).floor() as f32);
                        let max = [p[0] + s[0], p[1] + s[1], p[2] + s[2]];
                        if rng.next() < 0.5 {
                            bodies.push(make_box(p, max));
                        } else {
                            bodies.push(grid_box(p, max, 1 + (rng.next() * 3.0) as usize));
                        }
                    }
                }
                1 => {
                    // Generic: rotated boxes and spheres.
                    for _ in 0..4 {
                        let c = [0, 1, 2].map(|_| rng.next() * 12.0);
                        let h = [0, 1, 2].map(|_| 2.0 + rng.next() * 4.0);
                        bodies.push(rotated_box(c, h, rng.quaternion()));
                    }
                    for _ in 0..2 {
                        let c = [0, 1, 2].map(|_| rng.next() * 12.0);
                        bodies.push(sphere(c, 2.0 + rng.next() * 4.0, 10, 14));
                    }
                }
                2 => {
                    // A body and a slightly turned copy: nearly coplanar faces everywhere.
                    let c = [5.0, 5.0, 5.0];
                    let q = rng.quaternion();
                    bodies.push(rotated_box(c, [4.0, 3.0, 2.0], q));
                    let tiny = 1e-4 + rng.next() * 1e-3;
                    let turn = [
                        1.0,
                        tiny * (rng.next() - 0.5),
                        tiny * (rng.next() - 0.5),
                        tiny,
                    ];
                    let n = turn.iter().map(|x| x * x).sum::<f64>().sqrt();
                    let turn = turn.map(|x| x / n);
                    let qq = {
                        let [a1, b1, c1, d1] = turn;
                        let [a2, b2, c2, d2] = q;
                        [
                            a1 * a2 - b1 * b2 - c1 * c2 - d1 * d2,
                            a1 * b2 + b1 * a2 + c1 * d2 - d1 * c2,
                            a1 * c2 - b1 * d2 + c1 * a2 + d1 * b2,
                            a1 * d2 + b1 * c2 - c1 * b2 + d1 * a2,
                        ]
                    };
                    bodies.push(rotated_box(c, [4.0, 3.0, 2.0], qq));
                }
                3 => {
                    // Grid boxes stacked face to face with different subdivisions, plus one
                    // crossing them.
                    let n1 = 1 + (rng.next() * 3.0) as usize;
                    let n2 = 1 + (rng.next() * 3.0) as usize;
                    bodies.push(grid_box([0.0; 3], [4.0, 4.0, 4.0], n1));
                    bodies.push(grid_box([4.0, 1.0, 1.0], [7.0, 3.0, 3.0], n2));
                    bodies.push(make_box([2.0, 2.0, -1.0], [5.0, 3.0, 5.0]));
                }
                4 => {
                    // Unit cubes on a grid, touching by faces, edges and corners.
                    for _ in 0..10 {
                        let p: [f32; 3] = [0, 1, 2].map(|_| (rng.next() * 3.0).floor() as f32);
                        if !bodies.iter().any(|b| b.vertex(0) == p) {
                            bodies.push(make_box(p, [p[0] + 1.0, p[1] + 1.0, p[2] + 1.0]));
                        }
                    }
                }
                5 => {
                    // The same box twice with different triangulations, and a third crossing.
                    let n = 2 + (rng.next() * 2.0) as usize;
                    bodies.push(make_box([0.0; 3], [4.0, 3.0, 2.0]));
                    bodies.push(grid_box([0.0; 3], [4.0, 3.0, 2.0], n));
                    let c = [0, 1, 2].map(|_| 1.0 + rng.next() * 2.0);
                    bodies.push(rotated_box(c, [1.5, 1.0, 2.0], rng.quaternion()));
                }
                6 => {
                    // A box and copies shifted by a hair: nearly coincident surfaces.
                    let q = rng.quaternion();
                    let c = [3.0, 2.0, 1.0];
                    bodies.push(rotated_box(c, [2.0, 1.5, 1.0], q));
                    let shift = 1e-5 * (1.0 + rng.next());
                    bodies.push(rotated_box(
                        [c[0] + shift, c[1], c[2] - shift],
                        [2.0, 1.5, 1.0],
                        q,
                    ));
                    bodies.push(sphere([3.5, 2.0, 1.5], 1.2, 8, 12));
                }
                _ => {
                    // A box resting on the floor of a cavity in another box, poking out through the
                    // cavity's wall into the outside.
                    let outer = make_box([0.0; 3], [10.0; 3]);
                    let mut cavity = make_box([2.0; 3], [8.0; 3]);
                    for t in cavity.indices.as_chunks_mut::<3>().0 {
                        t.swap(1, 2);
                    }
                    bodies.push(concat(&[&outer, &cavity]).unwrap());
                    let h = 1.0 + (rng.next() * 3.0).floor() as f32;
                    bodies.push(make_box([4.0, 2.0, 4.0], [12.0, 2.0 + h, 6.0]));
                }
            }
            let refs: Vec<&Mesh> = bodies.iter().collect();
            let input = concat(&refs).unwrap();
            if let Ok(only) = std::env::var("STRESS_SCENE")
                && only.parse::<usize>().ok() != Some(scene)
            {
                continue;
            }
            let out = match silent(|c| run(&input, &[], c)) {
                Ok(out) => out,
                Err(e) => {
                    failed += 1;
                    eprintln!("scene {scene} (kind {kind}): error {e:?}");
                    continue;
                }
            };
            let c = silent(|control| check(&out.mesh, control)).unwrap();
            let flat = out
                .mesh
                .indices
                .as_chunks::<3>()
                .0
                .iter()
                .filter(|t| {
                    exact::collinear(
                        out.mesh.point(t[0]),
                        out.mesh.point(t[1]),
                        out.mesh.point(t[2]),
                    )
                })
                .count();
            if std::env::var("STRESS_SCENE").is_ok() {
                for b in &bodies {
                    eprintln!("  body {:?}", crate::mesh::bounds(b));
                }
                for (t, tri) in out.mesh.indices.as_chunks::<3>().0.iter().enumerate() {
                    let p = tri.map(|v| out.mesh.point(v));
                    if exact::collinear(p[0], p[1], p[2]) {
                        eprintln!("  flat {t} {tri:?} {:?}", tri.map(|v| out.mesh.vertex(v)));
                    }
                }
                for (i, j) in silent(|c| tritri::intersecting_pairs(&out.mesh, c)).unwrap() {
                    let tris = out.mesh.indices.as_chunks::<3>().0;
                    eprintln!(
                        "  crossing {i} {:?} / {j} {:?}",
                        tris[i as usize].map(|v| out.mesh.vertex(v)),
                        tris[j as usize].map(|v| out.mesh.vertex(v))
                    );
                }
            }
            if c != Check::default() || !out.warnings.is_empty() || flat > 0 {
                failed += 1;
                eprintln!(
                    "scene {scene} (kind {kind}): {c:?}, {flat} flat, warnings {:?}, {} -> {} triangles",
                    out.warnings,
                    input.triangle_count(),
                    out.mesh.triangle_count()
                );
                continue;
            }
            // The output bounds the union of the bodies, cavities filled.
            if kind == 7 {
                bodies[0] = make_box([0.0; 3], [10.0; 3]);
            }
            let wrapped = Solid::from_mesh(&out.mesh, None);
            let solids: Vec<Solid> = bodies.iter().map(|b| Solid::from_mesh(b, None)).collect();
            let [lo, hi] = crate::mesh::bounds(&input).unwrap();
            let input_tris = input.indices.as_chunks::<3>().0;
            let mut wrong = 0;
            for _ in 0..500 {
                let p: P3 = [0, 1, 2].map(|k| {
                    let (a, b) = (lo[k] as f64 - 0.5, hi[k] as f64 + 0.5);
                    a + (b - a) * rng.next()
                });
                // Rounding to the grid moves the surface by up to a step.
                if input_tris
                    .iter()
                    .any(|t| point_triangle_distance(p, t.map(|v| input.point(v))) < GRID)
                {
                    continue;
                }
                let (Some(w), Some(inside)) = (
                    wrapped.winding(p),
                    solids
                        .iter()
                        .try_fold(false, |acc, s| s.winding(p).map(|w| acc || w != 0)),
                ) else {
                    continue;
                };
                if w != inside as i32 {
                    wrong += 1;
                }
            }
            if wrong > 0 {
                failed += 1;
                eprintln!("scene {scene} (kind {kind}): {wrong} samples on the wrong side");
            }
            match silent(|c| run(&out.mesh, &[], c)) {
                Ok(again) if !again.changed => {}
                Ok(again) => {
                    failed += 1;
                    eprintln!(
                        "scene {scene} (kind {kind}): second run changed it: {}",
                        again.message
                    );
                }
                Err(e) => {
                    failed += 1;
                    eprintln!("scene {scene} (kind {kind}): second run error {e:?}");
                }
            }
        }
        eprintln!("{failed} failed, {:.2?} total", started.elapsed());
        assert_eq!(failed, 0);
    }

    /// A model file as the app opens it: every part baked into world space, all in one mesh.
    fn load_model(path: &str) -> Option<Mesh> {
        let mut progress = crate::progress::Progress::silent();
        let imported =
            match crate::formats::import_file(std::path::Path::new(path), true, &mut progress) {
                Ok(i) => i,
                Err(e) => {
                    eprintln!("{path}: {e}");
                    return None;
                }
            };
        let baked: Vec<Mesh> = imported
            .parts
            .iter()
            .map(|p| crate::mesh::bake(&p.mesh, &p.matrix.unwrap_or(crate::mesh::IDENTITY)))
            .collect();
        let refs: Vec<&Mesh> = baked.iter().collect();
        concat(&refs).ok()
    }

    /// Topology counts of a mesh for the logs below: border, non-manifold and misoriented edges,
    /// triangles that repeat an index or are flat.
    fn diag(mesh: &Mesh) -> String {
        let edges = Edges::new(mesh);
        let (mut border, mut nm, mut mis) = (0, 0, 0);
        for e in 0..edges.len() as u32 {
            let d = edges.degree(e);
            if d == 1 {
                border += 1;
            } else if d > 2 {
                nm += 1;
            } else if !topology::is_consistent(mesh, &edges, e) {
                mis += 1;
            }
        }
        let tris = mesh.indices.as_chunks::<3>().0;
        let repeated = tris.iter().filter(|&&t| !distinct(t)).count();
        let flat = tris
            .iter()
            .filter(|&&t| {
                distinct(t)
                    && exact::collinear(mesh.point(t[0]), mesh.point(t[1]), mesh.point(t[2]))
            })
            .count();
        let s = stats(mesh);
        format!(
            "{} triangles, {} shells, volume {:.3}, border {border}, non-manifold {nm}, \
             misoriented {mis}, repeated {repeated}, flat {flat}",
            tris.len(),
            s.shells,
            s.volume
        )
    }

    /// The shells (joined through vertices) of a mesh, largest first: triangles and volume, and how
    /// many of them share a vertex position with another shell.
    fn shell_summary(mesh: &Mesh) -> String {
        let edges = Edges::new(mesh);
        let (shell_of, shells) = topology::components(mesh, &edges, Link::Vertex);
        let mut size = vec![0usize; shells];
        let mut volume = vec![0.0f64; shells];
        let mut area = vec![0.0f64; shells];
        for (t, tri) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
            let s = shell_of[t] as usize;
            let p = tri.map(|v| mesh.point(v));
            size[s] += 1;
            volume[s] += exact::dot(p[0], exact::cross(p[1], p[2])) / 6.0;
            area[s] += exact::length(exact::normal(p[0], p[1], p[2])) / 2.0;
        }
        let mut owner: std::collections::HashMap<[u32; 3], usize> = Default::default();
        let mut touching = vec![false; shells];
        for (t, tri) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
            for &v in tri {
                let key = mesh.vertex(v).map(f32::to_bits);
                let s = shell_of[t] as usize;
                match owner.get(&key) {
                    Some(&o) if o != s => {
                        touching[o] = true;
                        touching[s] = true;
                    }
                    Some(_) => {}
                    None => {
                        owner.insert(key, s);
                    }
                }
            }
        }
        let mut order: Vec<usize> = (0..shells).collect();
        order.sort_by_key(|&s| std::cmp::Reverse(size[s]));
        if std::env::var("WRAP_SHELLS").is_ok() {
            // Per smaller shell: its box and how many of its vertex positions another shell has.
            let mut boxes = vec![bvh::EMPTY; shells];
            let mut shared = vec![0usize; shells];
            for (t, tri) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
                let s = shell_of[t] as usize;
                for &v in tri {
                    bvh::include(&mut boxes[s], mesh.point(v));
                    let key = mesh.vertex(v).map(f32::to_bits);
                    if owner.get(&key).is_some_and(|&o| o != s) {
                        shared[s] += 1;
                    }
                }
            }
            let main = order[0];
            let tris = mesh.indices.as_chunks::<3>().0;
            let main_tris: Vec<[u32; 3]> = (0..tris.len())
                .filter(|&t| shell_of[t] as usize == main)
                .map(|t| tris[t])
                .collect();
            let points: Vec<P3> = (0..mesh.vertex_count() as u32)
                .map(|v| mesh.point(v))
                .collect();
            let tree = Bvh::new(&bvh::boxes_of(&points, &main_tris));
            for &s in order.iter().skip(1).take(12) {
                // The nearest the main shell comes to this shell's corners.
                let mut nearest = f64::INFINITY;
                for (t, tri) in tris.iter().enumerate() {
                    if shell_of[t] as usize != s {
                        continue;
                    }
                    for &v in tri {
                        let p = mesh.point(v);
                        let r = 0.05;
                        tree.query(&[p.map(|x| x - r), p.map(|x| x + r)], |u| {
                            let q = main_tris[u as usize].map(|w| mesh.point(w));
                            let d = point_triangle_distance(p, q);
                            if d == 0.0 && shared[s] == 0 && nearest > 0.0 {
                                eprintln!(
                                    "      shell {s} corner {p:?} lies on main face {q:?}; its \
                                     face {:?}",
                                    tri.map(|w| mesh.point(w))
                                );
                            }
                            nearest = nearest.min(d);
                        });
                    }
                }
                eprintln!(
                    "    shell {s}: {} triangles, volume {:.5}, area {:.5}, {} corners on another \
                     shell, nearest main surface {nearest:.2e}, box {:?}",
                    size[s], volume[s], area[s], shared[s], boxes[s]
                );
            }
        }
        let list: Vec<String> = order
            .iter()
            .take(12)
            .map(|&s| {
                format!(
                    "{} ({:.4}, thick {:.1e}{})",
                    size[s],
                    volume[s],
                    2.0 * volume[s] / area[s].max(f64::MIN_POSITIVE),
                    if touching[s] { ", touching" } else { "" }
                )
            })
            .collect();
        format!(
            "{shells}, {} touching another; largest {}",
            touching.iter().filter(|&&t| t).count(),
            list.join(", ")
        )
    }

    /// The distance from p to triangle q (closest point by regions, Ericson).
    fn point_triangle_distance(p: P3, q: [P3; 3]) -> f64 {
        let [a, b, c] = q;
        let (ab, ac, ap) = (exact::sub(b, a), exact::sub(c, a), exact::sub(p, a));
        let (d1, d2) = (exact::dot(ab, ap), exact::dot(ac, ap));
        let at = |x: P3| exact::length(exact::sub(p, x));
        if d1 <= 0.0 && d2 <= 0.0 {
            return at(a);
        }
        let bp = exact::sub(p, b);
        let (d3, d4) = (exact::dot(ab, bp), exact::dot(ac, bp));
        if d3 >= 0.0 && d4 <= d3 {
            return at(b);
        }
        let vc = d1 * d4 - d3 * d2;
        if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
            return at(exact::add(a, exact::scale(ab, d1 / (d1 - d3))));
        }
        let cp = exact::sub(p, c);
        let (d5, d6) = (exact::dot(ab, cp), exact::dot(ac, cp));
        if d6 >= 0.0 && d5 <= d6 {
            return at(c);
        }
        let vb = d5 * d2 - d1 * d6;
        if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
            return at(exact::add(a, exact::scale(ac, d2 / (d2 - d6))));
        }
        let va = d3 * d6 - d5 * d4;
        if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
            let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
            return at(exact::add(b, exact::scale(exact::sub(c, b), w)));
        }
        let denom = 1.0 / (va + vb + vc);
        let (v, w) = (vb * denom, vc * denom);
        at(exact::add(
            a,
            exact::add(exact::scale(ab, v), exact::scale(ac, w)),
        ))
    }

    /// The edges of WRAP_FILES' prepared meshes whose sides do not balance, with the triangles on
    /// them (for debugging the input wrap gets).
    #[test]
    #[ignore]
    fn prepared_defects() {
        let Ok(list) = std::env::var("WRAP_FILES") else {
            return;
        };
        for path in list.split('|').filter(|p| !p.is_empty()) {
            let Some(input) = load_model(path) else {
                continue;
            };
            let input_edges = Edges::new(&input);
            let mut input_unbalanced = 0;
            for e in 0..input_edges.len() as u32 {
                let [lo, _] = input_edges.ends(e);
                let f = input_edges
                    .sides(e)
                    .iter()
                    .filter(|&&s| topology::side_ends(&input, s).0 == lo)
                    .count();
                input_unbalanced += (f * 2 != input_edges.degree(e)) as usize;
            }
            let pre = silent(|c| prepare(&input, c)).unwrap();
            let m = &pre.mesh;
            let edges = Edges::new(m);
            let (shell_of, _) = topology::components(m, &edges, Link::Vertex);
            let mut shown = 0;
            let mut count = 0;
            for e in 0..edges.len() as u32 {
                let [lo, hi] = edges.ends(e);
                let sides = edges.sides(e);
                let forward = sides
                    .iter()
                    .filter(|&&s| topology::side_ends(m, s).0 == lo)
                    .count();
                if forward * 2 == sides.len() {
                    continue;
                }
                count += 1;
                if shown < 8 {
                    shown += 1;
                    eprintln!("  edge {lo}-{hi} {:?} – {:?}", m.vertex(lo), m.vertex(hi));
                    for &s in sides {
                        let t = s / 3;
                        eprintln!(
                            "    side {s} runs {} tri {t} shell {} origin {} {:?}",
                            if topology::side_ends(m, s).0 == lo {
                                "up"
                            } else {
                                "down"
                            },
                            shell_of[t as usize],
                            pre.origin[t as usize] as i64,
                            m.indices[t as usize * 3..t as usize * 3 + 3]
                                .iter()
                                .map(|&v| m.vertex(v))
                                .collect::<Vec<_>>()
                        );
                    }
                }
            }
            eprintln!(
                "{path}: input {input_unbalanced} unbalanced edges, prepared {count}, holes left {}",
                pre.open_holes
            );
        }
    }

    /// Wraps each raw model of WRAP_PAIRS ("raw|reference" pairs separated by ';') and compares the
    /// result with the reference tool's by sampling points in their bounds: where one has material
    /// and the other not, grouped into 1 mm cells, with the winding number of the prepared mesh
    /// there. The files are only read.
    #[test]
    #[ignore]
    fn compare_with_reference() {
        let Ok(list) = std::env::var("WRAP_PAIRS") else {
            return;
        };
        let samples: usize = std::env::var("WRAP_SAMPLES").map_or(400_000, |s| s.parse().unwrap());
        for pair in list.split(';').filter(|p| !p.is_empty()) {
            let Some((raw, reference)) = pair.split_once('|') else {
                continue;
            };
            let (Some(input), Some(target)) = (load_model(raw), load_model(reference)) else {
                continue;
            };
            let out = silent(|c| super::wrap(&input, c, &mut Timings::default())).unwrap();
            let prepared = silent(|c| prepare(&input, c)).unwrap().mesh;
            let ours = Solid::from_mesh(&out.mesh, None);
            let theirs = Solid::from_mesh(&target, None);
            let given = Solid::from_mesh(&prepared, None);
            let raw_solid = Solid::from_mesh(&input, None);
            let [lo, hi] = crate::mesh::bounds(&target).unwrap();
            eprintln!(
                "  bounds: raw {:?}, reference {:?}",
                crate::mesh::bounds(&input),
                [lo, hi]
            );
            let size: P3 = [0, 1, 2].map(|k| (hi[k] - lo[k]) as f64);
            let cell_volume = size[0] * size[1] * size[2] / samples as f64;
            let mut rng = Rng(7);
            let mut cells: std::collections::BTreeMap<([i64; 3], bool), (usize, Vec<i32>)> =
                Default::default();
            let (mut missing, mut extra) = (0usize, 0usize);
            for _ in 0..samples {
                let p: P3 = [0, 1, 2].map(|k| lo[k] as f64 + size[k] * rng.next());
                let (Some(a), Some(b)) = (ours.winding(p), theirs.winding(p)) else {
                    continue;
                };
                if (a != 0) == (b != 0) {
                    continue;
                }
                let lacking = b != 0;
                if lacking {
                    missing += 1;
                } else {
                    extra += 1;
                }
                let key = (p.map(|x| x.floor() as i64), lacking);
                let entry = cells.entry(key).or_default();
                entry.0 += 1;
                if entry.1.len() < 8 {
                    entry.1.push(given.winding(p).unwrap_or(99));
                    entry.1.push(raw_solid.winding(p).unwrap_or(99));
                }
            }
            eprintln!(
                "{raw}\n  ours {}; missing {:.3} mm³, extra {:.3} mm³",
                diag(&out.mesh),
                missing as f64 * cell_volume,
                extra as f64 * cell_volume
            );
            let mut list: Vec<_> = cells.into_iter().collect();
            list.sort_by_key(|(_, (n, _))| std::cmp::Reverse(*n));
            for ((cell, lacking), (n, windings)) in list.iter().take(12) {
                eprintln!(
                    "    cell {cell:?} {}: {n} samples, prepared/raw winding {windings:?}",
                    if *lacking { "missing" } else { "extra" }
                );
            }
            // What the raw model and the reference have on a vertical line through the worst cell.
            if let Some(((cell, _), _)) = list.first() {
                // WRAP_ALONG="x,y" puts the line elsewhere.
                let (x, y) = std::env::var("WRAP_ALONG")
                    .ok()
                    .and_then(|s| {
                        let v: Vec<f64> =
                            s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                        (v.len() == 2).then(|| (v[0], v[1]))
                    })
                    .unwrap_or((cell[0] as f64 + 0.5, cell[1] as f64 + 0.5));
                for (label, m) in [
                    ("raw", &input),
                    ("prepared", &prepared),
                    ("reference", &target),
                    ("ours", &out.mesh),
                ] {
                    let tris = m.indices.as_chunks::<3>().0;
                    let edges = Edges::new(m);
                    let (shell_of, _) = topology::components(m, &edges, Link::Edge);
                    let mut hits: Vec<(f64, usize)> = Vec::new();
                    for (t, tri) in tris.iter().enumerate() {
                        let p = tri.map(|v| m.point(v));
                        let o = [0, 1, 2].map(|i| {
                            let (a, b) = (p[i], p[(i + 1) % 3]);
                            (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0])
                        });
                        if !(o.iter().all(|&s| s > 0.0) || o.iter().all(|&s| s < 0.0)) {
                            continue;
                        }
                        let n = exact::normal(p[0], p[1], p[2]);
                        let z = p[0][2] - (n[0] * (x - p[0][0]) + n[1] * (y - p[0][1])) / n[2];
                        hits.push((z, t));
                        let _ = shell_of[t];
                    }
                    hits.sort_by(|a, b| a.0.total_cmp(&b.0));
                    let list: Vec<String> = hits
                        .iter()
                        .map(|&(z, t)| {
                            let p = tris[t].map(|v| m.point(v));
                            let n = exact::normal(p[0], p[1], p[2]);
                            format!(
                                "{z:.4}{}(s{})",
                                if n[2] > 0.0 { "+" } else { "-" },
                                shell_of[t]
                            )
                        })
                        .collect();
                    eprintln!("    {label} along ({x}, {y}): {}", list.join(" "));
                    let mut seen: Vec<u32> = hits.iter().map(|&(_, t)| shell_of[t]).collect();
                    seen.sort_unstable();
                    seen.dedup();
                    for s in seen {
                        let members: Vec<usize> =
                            (0..tris.len()).filter(|&t| shell_of[t] == s).collect();
                        let mut b = bvh::EMPTY;
                        let mut volume = 0.0;
                        for &t in &members {
                            let p = tris[t].map(|v| m.point(v));
                            for q in p {
                                bvh::include(&mut b, q);
                            }
                            volume += volume6([0.0; 3], p) / 6.0;
                        }
                        let open = (0..edges.len() as u32)
                            .filter(|&e| {
                                edges.degree(e) != 2
                                    && shell_of[edges.sides(e)[0] as usize / 3] == s
                            })
                            .count();
                        eprintln!(
                            "      shell {s}: {} triangles, volume {volume:.4}, {open} open edges, \
                             box {b:?}",
                            members.len()
                        );
                    }
                }
            }
            // What the raw model has in the worst cell.
            if let Some(((cell, _), _)) = list.first() {
                let inside =
                    |p: P3| (0..3).all(|k| p[k] >= cell[k] as f64 && p[k] <= cell[k] as f64 + 1.0);
                let edges = Edges::new(&input);
                let mut border = 0;
                for e in 0..edges.len() as u32 {
                    let [a, b] = edges.ends(e);
                    if edges.degree(e) == 1 && (inside(input.point(a)) || inside(input.point(b))) {
                        border += 1;
                    }
                }
                let tris = input.indices.as_chunks::<3>().0;
                let near: Vec<usize> = (0..tris.len())
                    .filter(|&t| tris[t].iter().any(|&v| inside(input.point(v))))
                    .collect();
                eprintln!(
                    "    raw in that cell: {} triangles, {border} border edges",
                    near.len()
                );
                for &t in near.iter().take(6) {
                    let p = tris[t].map(|v| input.point(v));
                    eprintln!(
                        "      tri {t}: {p:?} normal {:?}",
                        exact::normal(p[0], p[1], p[2])
                    );
                }
            }
        }
    }

    /// Prints the topology counts and shells of the models listed in WRAP_FILES (for comparing with
    /// the reference tool's results). The files are only read.
    #[test]
    #[ignore]
    fn model_stats() {
        let Ok(list) = std::env::var("WRAP_FILES") else {
            return;
        };
        for path in list.split('|').filter(|p| !p.is_empty()) {
            let Some(mesh) = load_model(path) else {
                continue;
            };
            let c = silent(|control| check(&mesh, control)).unwrap();
            eprintln!(
                "{path}\n  {}\n  {c:?}\n  shells: {}",
                diag(&mesh),
                shell_summary(&mesh)
            );
            // WRAP_PROBE="x,y,z;x,y,z": the winding number at those points.
            if let Ok(probes) = std::env::var("WRAP_PROBE") {
                let solid = Solid::from_mesh(&mesh, None);
                for probe in probes.split(';') {
                    let p: Vec<f64> = probe
                        .split(',')
                        .filter_map(|x| x.trim().parse().ok())
                        .collect();
                    if p.len() == 3 {
                        eprintln!(
                            "  winding at {p:?}: {:?}",
                            solid.winding([p[0], p[1], p[2]])
                        );
                    }
                }
            }
        }
    }

    /// Wraps the models listed in WRAP_FILES (paths separated by '|') and prints what each stage
    /// took and what the result is:
    /// `WRAP_FILES="a.stl|b.stl" cargo test --release --lib wrap::tests::real_models -- --ignored
    /// --nocapture`. The files are only read.
    #[test]
    #[ignore]
    fn real_models() {
        let Ok(list) = std::env::var("WRAP_FILES") else {
            return;
        };
        for path in list.split('|').filter(|p| !p.is_empty()) {
            let Some(input) = load_model(path) else {
                continue;
            };
            eprintln!("{path}\n  in:  {}", diag(&input));
            let mut timings = Timings::default();
            let t0 = Instant::now();
            let out = silent(|c| super::wrap(&input, c, &mut timings)).unwrap();
            let took = t0.elapsed();
            let c = silent(|control| check(&out.mesh, control)).unwrap();
            eprintln!(
                "  out: {}\n       {c:?}\n       {} {:?}\n  took {took:.2?}: {timings:?}",
                diag(&out.mesh),
                out.message,
                out.warnings
            );
            eprintln!("  shells: {}", shell_summary(&out.mesh));
            let off_grid = (0..out.mesh.vertex_count() as u32)
                .filter(|&v| {
                    let p = out.mesh.vertex(v);
                    p != grid_point(grid_cell(p.map(f64::from))).map(|x| x as f32)
                })
                .count();
            eprintln!(
                "  {off_grid} of {} vertices off the grid",
                out.mesh.vertex_count()
            );
            // A second run must change nothing; WRAP_PASSES=1 skips it, more runs keep going.
            let passes: usize = std::env::var("WRAP_PASSES").map_or(2, |p| p.parse().unwrap_or(2));
            let mut current = out.mesh;
            for pass in 2..=passes {
                let t0 = Instant::now();
                let again = wrap(&current);
                let c = silent(|control| check(&again.mesh, control)).unwrap();
                eprintln!(
                    "  pass {pass}: changed {} ({:.2?}), {}; {c:?}",
                    again.changed,
                    t0.elapsed(),
                    again.message
                );
                if !again.changed {
                    break;
                }
                current = again.mesh;
            }
        }
    }
}
