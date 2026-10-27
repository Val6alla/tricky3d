//! Remove degenerate faces: collapse needles (an edge shorter than the tolerance) and remove
//! zero-area triangles, keeping the mesh connected and never turning a neighbour over. With a
//! selection, only selected triangles are collapsed.
//!
//! A triangle is degenerate when its shortest edge is below the tolerance (two of its corners lie
//! that close together) or it has no area at all: exactly the triangles the Status tab counts
//! (status::degenerate_triangles). A thin triangle with its corners far apart stays, as in the
//! reference tool's results. They are fixed one at a time, thinnest first:
//! - A needle (shortest edge below the tolerance) collapses that edge to its midpoint, or into the
//!   endpoint on a border or a sharp feature line when only one endpoint lies on one; when the
//!   midpoint is not allowed, into either endpoint.
//! - A cap (no area, with every edge long enough: its middle corner lies on its longest edge)
//!   splits the triangle across that edge in two at that corner (as if the edge were split there
//!   and the corner collapsed into the new vertex). Where that would leave a new degenerate
//!   triangle (the corner lies near an end of the edge), a short edge of the cap, at most twice
//!   the tolerance long, collapses instead.
//!
//! A degenerate sliver that rounding left turned over is fixed by the same rules. A zero-area
//! triangle that no fix may change is flipped into the triangle across its long edge between
//! passes, as Wrap part surface does before it starts (see `settled`).
//!
//! Every change keeps the surface a manifold (the link condition: no new non-manifold edge, no
//! pinched vertex, no hole), turns no remaining triangle over (its normal turns by less than 90°),
//! makes no good triangle degenerate, and moves no input vertex farther than the tolerance from
//! where it started, however many changes it takes part in. Spots that are already non-manifold or
//! inconsistently oriented are left alone. A triangle that cannot be fixed that way stays and is
//! reported. The queue runs until nothing more can change. The displacement bound can hold a fix
//! back because its vertices had moved before; a new pass measures the bound afresh from the
//! vertices' new positions, so without a selection passes repeat until one changes nothing, and a
//! second run finds nothing to do.
//!
//! Nothing a fix changes may cross the rest of the surface (tritri's rule, exact). Each fix's new
//! triangles are tested against each other and the triangles beside them before it is made, and
//! after the pass every changed triangle is tested against the whole surface; where one crosses,
//! the pass runs again from the input with those corners kept still, and after a few attempts with
//! their neighbours kept still too. A pass stands only when it makes no new crossing; when none
//! manages, the mesh stays as it was. Crossings that continue ones the input already had do not
//! count.
//!
//! The mesh is edited in place as triangles with a twin per side (the side across the same edge,
//! running the other way), so a vertex's fan is a walk from side to side and every check and change
//! touches only the triangles round the corners involved.

use std::cmp::Ordering;
use std::sync::atomic::Ordering as AtomicOrdering;

use rayon::prelude::*;

use super::bvh::{Aabb, Bvh, EMPTY, include};
use super::defaults::effective_degenerate_tolerance;
use super::exact::{P3, add, collinear, corners as input_corners, cross, dot, length, scale, sub};
use super::status::f32_slack;
use super::topology::{Edges, NONE};
use super::tritri::intersect;
use super::{Concern, Control, DegenerateParams, Origin, Outcome, RepairResult};
use crate::mesh::{Mesh, fmt_count};

/// cos 30°: two triangles meeting at a sharper angle make their edge a feature line, and a border
/// that turns by more makes a corner; needle collapses keep both in place.
const SHARP: f64 = 0.866_025_403_784_438_6;
/// A triangle whose height is below this share of its coordinates' size is within a few f32
/// rounding steps of flat, so its normal may point anywhere: Wrap part surface's rounding leaves
/// such slivers turned over on flat faces. The checks measure it against its neighbours instead.
const NOISE: f64 = 1.0 / (1u64 << 20) as f64;
/// Height bands of the queue below the tolerance. Within a band triangles go roughly in mesh
/// order, which keeps the work on a big mesh local; more bands would order them more finely and
/// jump about the mesh at every step.
const BANDS: usize = 64;
/// Queue steps between cancel checks.
const CHECK_EVERY: usize = 1 << 12;
/// Attempts before the region kept still widens (see `work`): each after the first keeps still the
/// corners where the one before made the surface cross itself.
const ATTEMPTS: usize = 6;

/// `twin` of a side with no triangle across (a border edge).
const BORDER: u32 = NONE;
/// `twin` of a side on a non-manifold or inconsistently oriented edge, which no fix touches.
const BAD: u32 = NONE - 1;
/// Marks a removed triangle (its first corner).
const DEAD: u32 = NONE;

// Triangle states while the queue runs.
/// Not queued: not degenerate, fixed, or not tried yet.
const IDLE: u8 = 0;
const QUEUED: u8 = 1;
/// Tried and failed because of the link condition or a shape check, which a change round its
/// corners can lift: tried again in the next round when that happens.
const STUCK: u8 = 2;
/// Tried and failed only because every fix within reach would take an input vertex that moved
/// before farther than the tolerance from where it started, which only a change to the triangle
/// itself can lift.
const SPENT: u8 = 3;
/// Tried and failed with the displacement bound in the way of some fix: tried again like a stuck
/// one, and reported with the spent ones, since a new run (which measures the bound afresh) may
/// fix it.
const HELD: u8 = 4;
/// Tried and failed only because a corner's fan or an edge it needs is non-manifold or
/// inconsistently oriented: tried again only once that is no longer so.
const TANGLED: u8 = 5;

pub fn run(
    mesh: &Mesh,
    selection: &[u32],
    params: &DegenerateParams,
    control: &mut Control,
) -> RepairResult<Outcome> {
    let tolerance = effective_degenerate_tolerance(params);
    let only = (!selection.is_empty()).then_some(selection);
    let w = if only.is_some() {
        work(mesh, tolerance, only, control)?
    } else {
        settled(mesh, tolerance, control)?
    };
    let mut warnings = Vec::new();
    let left = Concern::Degenerate(tolerance);
    if w.bound > 0 {
        warnings.push(format!(
            "{} left because fixing {} would move the surface by more than the tolerance \
             ({tolerance:.5} mm).",
            faces_were(w.bound),
            them(w.bound)
        ));
    }
    if w.tangled > 0 {
        warnings.push(format!(
            "{} left where the mesh is non-manifold or inconsistently oriented.",
            faces_were(w.tangled)
        ));
    }
    if w.stuck > 0 {
        warnings.push(format!(
            "{} left because fixing {} would tear or fold the mesh.",
            faces_were(w.stuck),
            them(w.stuck)
        ));
    }
    if w.crossing > 0 {
        warnings.push(format!(
            "{} left because fixing {} would make the surface cross itself.",
            faces_were(w.crossing),
            them(w.crossing)
        ));
    }
    let concerns: Vec<(usize, Concern)> = (0..warnings.len()).map(|i| (i, left)).collect();
    if !w.changed {
        let message = if w.found == 0 {
            "No degenerate faces to remove."
        } else {
            "No degenerate faces could be removed."
        };
        let mut out = Outcome::unchanged(mesh, message);
        out.warnings = warnings;
        out.concerns = concerns;
        return Ok(out);
    }
    let fixed = w.found.saturating_sub(w.left());
    let message = if fixed == 0 {
        "Reshaped the mesh, but no degenerate face could be removed.".to_string()
    } else {
        format!("Removed {}.", faces(fixed))
    };
    Ok(Outcome {
        mesh: w.mesh,
        changed: true,
        origin: w.origin,
        message,
        warnings,
        highlight: None,
        select: None,
        concerns,
    })
}

/// A collapse pass's result.
#[derive(Clone, Debug, PartialEq)]
pub struct Collapsed {
    pub mesh: Mesh,
    pub origin: Origin,
    /// Triangles removed.
    pub removed: usize,
    /// Degenerate triangles that could not be fixed without breaking the mesh.
    pub left: usize,
}

/// Collapses every degenerate triangle at `tolerance` mm (absolute) among `only` (all when None).
pub fn collapse(
    mesh: &Mesh,
    tolerance: f64,
    only: Option<&[u32]>,
    control: &mut Control,
) -> RepairResult<Collapsed> {
    let w = work(mesh, tolerance, only, control)?;
    Ok(Collapsed {
        left: w.left(),
        mesh: w.mesh,
        origin: w.origin,
        removed: w.removed,
    })
}

/// Passes at most when the whole mesh is fixed (see `settled`).
const PASSES: usize = 8;

/// Fixes every degenerate triangle of the whole mesh, pass after pass until a pass changes nothing.
/// A fix that the displacement bound held back can be made by a later pass, which measures the
/// bound afresh from where the vertices are now; a second run of the action, or of a script, is
/// exactly such a pass, so repeating them here is what makes that second run change nothing. Each
/// pass moves an input vertex by at most the tolerance, and later passes fix only the few faces the
/// bound held back.
///
/// Before each later pass, the zero-area triangles left (a fix may leave a neighbour exactly flat,
/// and a zero-area triangle whose flip would leave thin triangles stays) are flipped into the
/// triangle across their long edge, as Wrap part surface does before it starts, so Wrap finds
/// nothing to flip after this action. The flip moves no vertex and its two triangles cover exactly
/// the one across, but next to coincident vertices it can still fold two triangles onto each other
/// (996802.stl of Thingi10K): flips that make the surface cross itself are not made.
fn settled(mesh: &Mesh, tolerance: f64, control: &mut Control) -> RepairResult<Work> {
    let mut w = work(mesh, tolerance, None, &mut control.sub(0.0, 0.9))?;
    let mut passes = 1;
    while passes < PASSES {
        // Round coincident vertices the flips can fold two triangles onto each other: then they
        // wait (this action must not make the surface cross itself).
        let zipped = super::wrap::zip_collinear(&w.mesh)
            .filter(|(flipped, _)| !flips_cross(&w.mesh, flipped));
        if !w.changed && zipped.is_none() {
            break;
        }
        if let Some((flipped, map)) = zipped {
            w.origin = super::compose_origin(&w.origin, &Origin::Map(map));
            w.mesh = flipped;
            w.changed = true;
        }
        // Each pass takes half of what is left of the progress bar.
        let done = 1.0 - 0.5f64.powi(passes as i32 - 1);
        let next = work(
            &w.mesh,
            tolerance,
            None,
            &mut control.sub(0.9 + 0.1 * done, 0.9 + 0.1 * (done + 1.0) / 2.0),
        )?;
        passes += 1;
        // What is left is what the last pass found left.
        if !next.changed {
            w = Work {
                mesh: w.mesh,
                origin: w.origin,
                changed: w.changed,
                found: w.found,
                removed: w.removed,
                #[cfg(test)]
                vertex_of: w.vertex_of,
                ..next
            };
            break;
        }
        #[cfg(test)]
        let vertex_of = w
            .vertex_of
            .iter()
            .map(|&v| {
                if v == NONE {
                    NONE
                } else {
                    next.vertex_of[v as usize]
                }
            })
            .collect();
        w = Work {
            origin: super::compose_origin(&w.origin, &next.origin),
            removed: w.removed + next.removed,
            found: w.found,
            #[cfg(test)]
            vertex_of,
            ..next
        };
    }
    control.progress(1.0);
    Ok(w)
}

/// True when a triangle that differs between `before` and `after` (the same triangle count, as
/// zip_collinear leaves it) crosses another triangle of `after` (tritri's rule, exact).
fn flips_cross(before: &Mesh, after: &Mesh) -> bool {
    let changed: Vec<u32> = (0..after.triangle_count())
        .filter(|&t| after.indices[t * 3..t * 3 + 3] != before.indices[t * 3..t * 3 + 3])
        .map(|t| t as u32)
        .collect();
    let box_of = |p: [P3; 3]| {
        let mut b = EMPTY;
        for q in p {
            include(&mut b, q);
        }
        b
    };
    let bvh = Bvh::new(
        &changed
            .iter()
            .map(|&t| box_of(input_corners(after, t)))
            .collect::<Vec<Aabb>>(),
    );
    (0..after.triangle_count() as u32).into_par_iter().any(|g| {
        let pg = input_corners(after, g);
        let mut hit = false;
        bvh.query(&box_of(pg), |i| {
            let t = changed[i as usize];
            hit |= t != g && intersect(&input_corners(after, t), &pg);
        });
        hit
    })
}

/// "1 degenerate face", "1,204 degenerate faces".
fn faces(n: usize) -> String {
    let unit = if n == 1 { "face" } else { "faces" };
    format!("{} degenerate {unit}", fmt_count(n as u64))
}

/// "1 degenerate face was", "3 degenerate faces were".
fn faces_were(n: usize) -> String {
    format!("{} {}", faces(n), if n == 1 { "was" } else { "were" })
}

fn them(n: usize) -> &'static str {
    if n == 1 { "it" } else { "them" }
}

// ---------------------------------------------------------------------------------------------
// The pass

/// What one pass did.
struct Work {
    mesh: Mesh,
    origin: Origin,
    changed: bool,
    /// Degenerate triangles among the allowed ones at the start.
    found: usize,
    /// Triangles removed.
    removed: usize,
    /// Degenerate triangles left: the displacement bound stood in the way of a fix (`bound`), they
    /// touch a non-manifold or inconsistently oriented spot (`tangled`), or the link condition and
    /// the shape checks stood in the way (`stuck`).
    bound: usize,
    tangled: usize,
    stuck: usize,
    /// Degenerate triangles left at vertices an earlier attempt made the surface cross itself at.
    crossing: usize,
    /// For each input vertex, the output vertex it ended up in (NONE when no triangle uses it), for
    /// the tests that check the displacement bound.
    #[cfg(test)]
    vertex_of: Vec<u32>,
}

impl Work {
    fn left(&self) -> usize {
        self.bound + self.tangled + self.stuck + self.crossing
    }
}

fn work(
    mesh: &Mesh,
    tolerance: f64,
    only: Option<&[u32]>,
    control: &mut Control,
) -> RepairResult<Work> {
    control.check()?;
    let count = mesh.triangle_count();
    let allowed: Option<Vec<bool>> = only.map(|selection| {
        let mut mask = vec![false; count];
        for &t in selection {
            if let Some(m) = mask.get_mut(t as usize) {
                *m = true;
            }
        }
        mask
    });
    // One parallel pass finds the candidates and their heights (−1 for a repeated corner); a mesh
    // without any is done after it.
    let found: Vec<(u32, f64)> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .par_iter()
        .enumerate()
        .filter_map(|(t, c)| {
            if allowed.as_ref().is_some_and(|a| !a[t]) {
                return None;
            }
            if repeated(*c) {
                return Some((t as u32, -1.0));
            }
            let s = shape(c.map(|v| mesh.point(v)), tolerance);
            s.degenerate.then_some((t as u32, s.height))
        })
        .collect();
    if found.is_empty() {
        return Ok(Work {
            mesh: mesh.clone(),
            origin: Origin::Same,
            changed: false,
            found: 0,
            removed: 0,
            bound: 0,
            tangled: 0,
            stuck: 0,
            crossing: 0,
            #[cfg(test)]
            vertex_of: (0..mesh.vertex_count() as u32).collect(),
        });
    }
    control.progress(0.05);
    // Attempts: a pass, then the check that nothing it changed or made crosses another triangle.
    // Where something does, the input vertices involved stay put in the next attempt, which starts
    // from the input again; from the ATTEMPTS-th on, so do the other corners of their input
    // triangles, which widens the still region each time. An attempt stands only when it crosses
    // nothing. When none has after twice ATTEMPTS, nothing changes and every degenerate face is
    // reported as left to keep the surface from crossing itself.
    let mut frozen = vec![false; mesh.vertex_count()];
    // The editable surface is built once; each attempt starts from a copy of it.
    let mut base = Surface::new(mesh, tolerance, allowed, &found, &frozen);
    base.stall_limit = found.len() / 4 + 16;
    let last = 2 * ATTEMPTS;
    let mut attempt = 0;
    loop {
        if attempt == last {
            base.frozen.fill(true);
            control.progress(1.0);
            return Ok(base.finish(mesh, found.len()));
        }
        let (start, end) = if attempt == 0 {
            (0.05, 0.85)
        } else {
            let step = 0.1 / (last - 1) as f64;
            (
                0.85 + step * (attempt - 1) as f64,
                0.85 + step * attempt as f64,
            )
        };
        let mut s = base.clone();
        s.frozen.clone_from(&frozen);
        let (crossing, _) = {
            let mut step = control.sub(start, end);
            step.check()?;
            s.settle(&mut step)?;
            if cfg!(debug_assertions) {
                s.audit();
            }
            s.crossings(mesh, &step)?
        };
        attempt += 1;
        if crossing.is_empty() {
            control.progress(0.95);
            let w = s.finish(mesh, found.len());
            control.progress(1.0);
            return Ok(w);
        }
        for v in crossing {
            frozen[v as usize] = true;
        }
        if attempt >= ATTEMPTS {
            let mut wider = frozen.clone();
            for c in mesh.indices.as_chunks::<3>().0 {
                if c.iter().any(|&v| frozen[v as usize]) {
                    for &v in c {
                        wider[v as usize] = true;
                    }
                }
            }
            frozen = wider;
        }
    }
}

fn repeated(c: [u32; 3]) -> bool {
    c[0] == c[1] || c[1] == c[2] || c[2] == c[0]
}

/// What the checks need to know about a triangle's shape.
#[derive(Clone, Copy)]
struct Shape {
    /// Degenerate by the Status tab's rule (status::degenerate_triangles).
    degenerate: bool,
    /// (b − a) × (c − a): along the normal, twice the area long.
    normal: P3,
    /// Over the longest edge.
    height: f64,
    /// The normal is far enough above rounding noise to compare it with others.
    trusted: bool,
}

/// The shape of a triangle with these corners, in the triangle's own corner order. The degenerate
/// test is status::degenerate's, operation for operation, so both always agree.
fn shape([a, b, c]: [P3; 3], tolerance: f64) -> Shape {
    let lengths = [sub(b, a), sub(c, b), sub(a, c)].map(|d| dot(d, d));
    let shortest = lengths[0].min(lengths[1]).min(lengths[2]).sqrt();
    let longest = lengths[0].max(lengths[1]).max(lengths[2]).sqrt();
    let n = cross(sub(b, a), sub(c, a));
    let twice_area = dot(n, n).sqrt();
    let degenerate = shortest < tolerance - f32_slack([a, b, c])
        || (twice_area <= 1e-9 * longest * longest && collinear(a, b, c));
    let height = if longest > 0.0 {
        twice_area / longest
    } else {
        0.0
    };
    let size = [a, b, c]
        .iter()
        .flatten()
        .fold(0.0f64, |m, x| m.max(x.abs()));
    Shape {
        degenerate,
        normal: n,
        height,
        trusted: height > size * NOISE,
    }
}

/// Whether a triangle may change from `old` to `new`: a good triangle stays good and its normal
/// turns by less than 90°; a degenerate one that faces the way of its neighbourhood (`reference`)
/// keeps facing it, or becomes exactly flat; one that faces away (turned over, or flat within
/// rounding) may stay degenerate any way, but becomes good only facing the neighbourhood's way.
fn keeps(old: &Shape, new: &Shape, reference: P3) -> bool {
    if !old.degenerate {
        return !new.degenerate && dot(new.normal, old.normal) > 0.0;
    }
    if old.trusted && dot(old.normal, reference) > 0.0 {
        return new.normal == [0.0; 3] || dot(new.normal, old.normal) > 0.0;
    }
    new.degenerate || dot(new.normal, reference) > 0.0
}

/// The point at `s` along a → b, exactly a or b at the ends.
fn lerp(a: P3, b: P3, s: f64) -> P3 {
    if s == 0.0 {
        a
    } else if s == 1.0 {
        b
    } else {
        add(a, scale(sub(b, a), s))
    }
}

/// The nearest f32 position, as the output stores it: every check runs on the rounded point.
fn round32(p: P3) -> P3 {
    p.map(|x| x as f32 as f64)
}

/// The smallest sphere holding two spheres (centre, radius).
fn enclose(c1: P3, r1: f64, c2: P3, r2: f64) -> (P3, f64) {
    let d = length(sub(c2, c1));
    if d + r2 <= r1 {
        return (c1, r1);
    }
    if d + r1 <= r2 {
        return (c2, r2);
    }
    let r = (d + r1 + r2) / 2.0;
    (add(c1, scale(sub(c2, c1), (r - r1) / d)), r)
}

/// True when triangles with corners `a` and `b` (positions `pa`, `pb`) share an edge that they run
/// opposite ways and face the same way: then they meet only along that edge and cannot cross,
/// whatever the rest of their shape, which spares the exact test for most neighbours.
fn hinged(a: [u32; 3], b: [u32; 3], pa: &[P3; 3], pb: &[P3; 3]) -> bool {
    let shared = (0..3).any(|k| {
        let (x, y) = (a[k], a[(k + 1) % 3]);
        (0..3).any(|j| b[j] == y && b[(j + 1) % 3] == x)
    });
    shared && {
        let na = cross(sub(pa[1], pa[0]), sub(pa[2], pa[0]));
        let nb = cross(sub(pb[1], pb[0]), sub(pb[2], pb[0]));
        dot(na, nb) > 0.0
    }
}

/// Triangle `t`'s corners in `mesh`.
fn input_triangle(mesh: &Mesh, t: u32) -> [u32; 3] {
    let i = t as usize * 3;
    [mesh.indices[i], mesh.indices[i + 1], mesh.indices[i + 2]]
}

/// The next side of the same triangle (the side leaving the corner this one ends at).
#[inline]
fn next(s: u32) -> u32 {
    s - s % 3 + (s + 1) % 3
}

/// The previous side of the same triangle (the side arriving at the corner this one leaves).
#[inline]
fn prev(s: u32) -> u32 {
    s - s % 3 + (s + 2) % 3
}

/// One attempt's result. Combining several: Spent or Tangled only when all were, Stuck when the
/// others were Stuck or Tangled, else Held.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fix {
    Done,
    /// The displacement bound alone stood in the way of every fix.
    Spent,
    /// The displacement bound stood in the way of some fix.
    Held,
    /// Only the link condition and the shape checks (and non-manifold spots) stood in the way.
    Stuck,
    /// Only non-manifold or inconsistently oriented spots stood in the way.
    Tangled,
}

impl Fix {
    fn and(self, other: Fix) -> Fix {
        match (self, other) {
            (Fix::Done, _) | (_, Fix::Done) => Fix::Done,
            (Fix::Spent, Fix::Spent) => Fix::Spent,
            (Fix::Tangled, Fix::Tangled) => Fix::Tangled,
            (Fix::Stuck | Fix::Tangled, Fix::Stuck | Fix::Tangled) => Fix::Stuck,
            _ => Fix::Held,
        }
    }
}

/// The attempts so far combined with one more.
fn join(so_far: Option<Fix>, r: Fix) -> Option<Fix> {
    Some(so_far.map_or(r, |x| x.and(r)))
}

/// Where in [0, 1] the convex `f` is smallest, by golden-section search.
fn argmin(f: impl Fn(f64) -> f64) -> f64 {
    const G: f64 = 0.618_033_988_749_894_9;
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    let (mut x1, mut x2) = (1.0 - G, G);
    let (mut f1, mut f2) = (f(x1), f(x2));
    for _ in 0..40 {
        if f1 < f2 {
            hi = x2;
            (x2, f2) = (x1, f1);
            x1 = hi - G * (hi - lo);
            f1 = f(x1);
        } else {
            lo = x1;
            (x1, f1) = (x2, f2);
            x2 = lo + G * (hi - lo);
            f2 = f(x2);
        }
    }
    (lo + hi) / 2.0
}

/// A priority queue of triangles by height, in bands below the tolerance: thinner bands first,
/// the last one in first within a band. Entries carry the triangle's stamp when they went in; the
/// caller skips those whose triangle changed since.
#[derive(Clone)]
struct Bands {
    bands: Vec<Vec<(u32, u32)>>,
    lowest: usize,
    scale: f64,
    len: usize,
}

impl Bands {
    fn new(tolerance: f64) -> Bands {
        Bands {
            bands: vec![Vec::new(); BANDS],
            lowest: BANDS,
            scale: if tolerance > 0.0 {
                BANDS as f64 / tolerance
            } else {
                0.0
            },
            len: 0,
        }
    }

    /// The band of a triangle this high.
    fn band(&self, height: f64) -> u8 {
        ((height * self.scale) as usize).min(BANDS - 1) as u8
    }

    fn push(&mut self, band: u8, t: u32, stamp: u32) {
        let band = band as usize;
        self.bands[band].push((t, stamp));
        self.lowest = self.lowest.min(band);
        self.len += 1;
    }

    fn pop(&mut self) -> Option<(u32, u32)> {
        while self.lowest < BANDS {
            if let Some(entry) = self.bands[self.lowest].pop() {
                self.len -= 1;
                return Some(entry);
            }
            self.lowest += 1;
        }
        None
    }
}

/// The triangles round one vertex, measured for the link condition and the shape checks.
#[derive(Default)]
struct Ring {
    /// The side leaving the vertex of each triangle of its fan, in turning order: each triangle's
    /// previous corner is the next one's next corner. An open fan starts at the border.
    sides: Vec<u32>,
    /// The fan's first side and its last triangle's side arriving at the vertex are border sides.
    open: bool,
    /// Per triangle: its shape before the change (filled by `measure`).
    shapes: Vec<Shape>,
    /// Border edges, sharp edges and border corners at the vertex: 0 none, 1 one or two (on a
    /// line), 2 more (a corner).
    rank: u8,
}

/// Buffers reused by every fix, so the queue does no allocation per triangle.
#[derive(Default)]
struct Scratch {
    /// The fans round the corners of the triangle being fixed, each walked when first needed in
    /// an attempt (`ring_of` holds the vertex, NONE before), whether it is one clean fan, and
    /// whether it was measured.
    rings: [Ring; 3],
    ring_of: [u32; 3],
    fan_ok: [bool; 3],
    measured: [bool; 3],
    /// The triangles the last fix changed or made.
    changed: Vec<u32>,
    check: Check,
}

/// What a fix would leave round the vertices it moves, for the fold test: the triangles it would
/// make or reshape, and the unchanged triangles across their outer edges (with the made triangle
/// each lies beside).
#[derive(Default)]
struct Check {
    made: Vec<([P3; 3], Shape)>,
    beside: Vec<(usize, [P3; 3], Shape)>,
}

impl Check {
    fn clear(&mut self) {
        self.made.clear();
        self.beside.clear();
    }

    /// True when a made triangle crosses another made one or a triangle beside it (tritri's rule,
    /// exact): the fix would fold the surface over itself. Made triangles all share a moved vertex
    /// and a triangle beside shares an edge with its made one; such a pair facing the same way can
    /// only cross when one of them turned over, which `keeps` refuses. So only the pairs facing
    /// apart, or with a normal that is rounding noise, need the exact test.
    fn folds(&self) -> bool {
        let suspect =
            |a: &Shape, b: &Shape| !a.trusted || !b.trusted || dot(a.normal, b.normal) < 0.0;
        for (i, (p, a)) in self.made.iter().enumerate() {
            for (r, b) in &self.made[i + 1..] {
                if suspect(a, b) && intersect(p, r) {
                    return true;
                }
            }
        }
        self.beside.iter().any(|(i, r, b)| {
            let (p, a) = &self.made[*i];
            suspect(a, b) && intersect(p, r)
        })
    }
}

/// The mesh while it is being edited: f64 positions that are always exact f32 values, triangles
/// with a DEAD mark for removed ones, and a twin per side.
#[derive(Clone)]
struct Surface<'a> {
    tolerance: f64,
    /// The input positions, which the displacement bound measures from.
    input: &'a [f32],
    pos: Vec<P3>,
    tri: Vec<[u32; 3]>,
    /// Per side (3 t + k runs from corner k to corner k + 1): the side across its edge, running the
    /// other way, or BORDER or BAD.
    twin: Vec<u32>,
    /// Input triangle of each triangle (a split's new half has its parent's).
    origin: Vec<u32>,
    /// Per input triangle: may be fixed (the selection); None when all may.
    allowed: Option<Vec<bool>>,
    /// Per vertex: a side leaving it (NONE when no triangle uses it) and its live triangles.
    out: Vec<u32>,
    valence: Vec<u32>,
    live: usize,
    /// Per vertex: the input vertices merged into it (its cluster), as a list through `next`
    /// starting at the vertex itself and ending at `tail`.
    next: Vec<u32>,
    tail: Vec<u32>,
    /// Per vertex: its cluster's input positions all lie within `reach` of `center`, so moving the
    /// vertex to q moves none of them farther than |q − center| + reach. Most bound checks need no
    /// more than that.
    center: Vec<P3>,
    reach: Vec<f64>,
    state: Vec<u8>,
    /// Per triangle: changes whenever the triangle does, so queue entries made before go stale.
    stamp: Vec<u32>,
    /// Per triangle: the queue band its entry is in, while it is queued.
    band: Vec<u8>,
    queue: Bands,
    /// Fixes made so far.
    epoch: u32,
    /// Per vertex: the epoch of the last fix that changed a triangle at it, and that epoch when its
    /// triangles were last found not to form one clean fan (NONE when they were not). A vertex's
    /// fan only changes with its triangles, so a triangle tried again skips walking it.
    changed_at: Vec<u32>,
    bad_ring: Vec<u32>,
    /// Per vertex: a mark from `new_mark`, for set tests among neighbours.
    mark: Vec<u32>,
    marker: u32,
    /// Triangles that failed in a way a change round their corners can lift (`listed` marks them),
    /// and per triangle the epoch it was last tried in. Every check on a triangle looks only at the
    /// triangles round its corners and the one across its long edge, which shares two corners.
    failed: Vec<u32>,
    listed: Vec<bool>,
    tried_at: Vec<u32>,
    /// Fixes that left as many degenerate triangles as before (a cap split whose triangle across
    /// was a cap over the same edge); bounded so the queue always ends.
    stalls: usize,
    stall_limit: usize,
    changed: bool,
    /// Per vertex: kept where it is, since an earlier attempt made the surface cross itself there.
    frozen: Vec<bool>,
    /// Per triangle: a fix changed or made it.
    touched: Vec<bool>,
}

impl<'a> Surface<'a> {
    /// Builds the editable mesh with the triangles of `found` that have a repeated corner already
    /// removed, the others queued, and the vertices `frozen` marks kept where they are.
    fn new(
        mesh: &'a Mesh,
        tolerance: f64,
        allowed: Option<Vec<bool>>,
        found: &[(u32, f64)],
        frozen: &[bool],
    ) -> Surface<'a> {
        let pos: Vec<P3> = mesh
            .positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| p.map(f64::from))
            .collect();
        let vertices = pos.len();
        let mut tri: Vec<[u32; 3]> = mesh.indices.as_chunks::<3>().0.to_vec();
        let triangles = tri.len();
        let mut live = triangles;
        let mut changed = false;
        for &(t, height) in found {
            if height < 0.0 {
                tri[t as usize][0] = DEAD;
                live -= 1;
                changed = true;
            }
        }
        // Twins from each edge's live sides: exactly two, of different triangles, running opposite
        // ways (starting at different vertices). Everything else stays BAD, except a lone side.
        let edges = Edges::new(mesh);
        let mut twin = vec![BAD; triangles * 3];
        for e in 0..edges.len() as u32 {
            let mut sides = edges
                .sides(e)
                .iter()
                .copied()
                .filter(|&s| tri[(s / 3) as usize][0] != DEAD);
            match (sides.next(), sides.next(), sides.next()) {
                (Some(s), None, _) => twin[s as usize] = BORDER,
                (Some(s), Some(r), None)
                    if s / 3 != r / 3 && mesh.indices[s as usize] != mesh.indices[r as usize] =>
                {
                    twin[s as usize] = r;
                    twin[r as usize] = s;
                }
                _ => {}
            }
        }
        drop(edges);
        let mut out = vec![NONE; vertices];
        let mut valence = vec![0u32; vertices];
        for (t, c) in tri.iter().enumerate() {
            if c[0] == DEAD {
                continue;
            }
            for (k, &v) in c.iter().enumerate() {
                valence[v as usize] += 1;
                out[v as usize] = (t * 3 + k) as u32;
            }
        }
        let mut s = Surface {
            tolerance,
            input: &mesh.positions,
            center: pos.clone(),
            pos,
            tri,
            twin,
            origin: (0..triangles as u32).collect(),
            allowed,
            out,
            valence,
            live,
            next: vec![NONE; vertices],
            tail: (0..vertices as u32).collect(),
            reach: vec![0.0; vertices],
            state: vec![IDLE; triangles],
            stamp: vec![0; triangles],
            band: vec![0; triangles],
            queue: Bands::new(tolerance),
            epoch: 0,
            changed_at: vec![0; vertices],
            bad_ring: vec![NONE; vertices],
            mark: vec![0; vertices],
            marker: 0,
            failed: Vec::new(),
            listed: vec![false; triangles],
            tried_at: vec![0; triangles],
            stalls: 0,
            stall_limit: 0,
            changed,
            frozen: frozen.to_vec(),
            touched: vec![false; triangles],
        };
        // A vertex on a non-manifold or inconsistently oriented edge has no clean fan: known from
        // the start, so the triangles whose every fix needs one are not even tried (unless that
        // changes).
        for side in 0..triangles * 3 {
            if s.twin[side] == BAD && s.tri[side / 3][0] != DEAD {
                let side = side as u32;
                for v in [s.start(side), s.end(side)] {
                    s.bad_ring[v as usize] = 0;
                }
            }
        }
        for &(t, height) in found {
            if height < 0.0 {
                continue;
            }
            if s.hopeless(t) {
                s.failed(t, Fix::Tangled);
            } else {
                let ti = t as usize;
                s.state[ti] = QUEUED;
                s.band[ti] = s.queue.band(height);
                s.queue.push(s.band[ti], t, 0);
            }
        }
        s
    }

    /// The vertex side `s` leaves.
    #[inline]
    fn start(&self, s: u32) -> u32 {
        self.tri[(s / 3) as usize][(s % 3) as usize]
    }

    /// The vertex side `s` arrives at.
    #[inline]
    fn end(&self, s: u32) -> u32 {
        self.tri[(s / 3) as usize][((s + 1) % 3) as usize]
    }

    fn alive(&self, t: u32) -> bool {
        self.tri[t as usize][0] != DEAD
    }

    fn allowed(&self, t: u32) -> bool {
        self.allowed
            .as_ref()
            .is_none_or(|a| a[self.origin[t as usize] as usize])
    }

    fn corners(&self, t: u32) -> [P3; 3] {
        self.tri[t as usize].map(|v| self.pos[v as usize])
    }

    fn input_at(&self, v: u32) -> P3 {
        let i = v as usize * 3;
        [0, 1, 2].map(|k| f64::from(self.input[i + k]))
    }

    fn kill(&mut self, t: u32) {
        self.tri[t as usize][0] = DEAD;
        self.live -= 1;
    }

    fn push_triangle(&mut self, corners: [u32; 3], origin: u32) -> u32 {
        let t = self.tri.len() as u32;
        self.tri.push(corners);
        self.twin.extend_from_slice(&[BAD; 3]);
        self.origin.push(origin);
        self.state.push(IDLE);
        self.stamp.push(0);
        self.band.push(0);
        self.listed.push(false);
        self.tried_at.push(0);
        self.touched.push(false);
        self.live += 1;
        t
    }

    /// Makes sides `x` and `y` (either may be BORDER) each other's twin.
    fn glue(&mut self, x: u32, y: u32) {
        if x < BAD {
            self.twin[x as usize] = y;
        }
        if y < BAD {
            self.twin[y as usize] = x;
        }
    }

    /// A fresh mark for `mark`.
    fn new_mark(&mut self) -> u32 {
        self.marker = self.marker.wrapping_add(1);
        if self.marker == 0 {
            self.mark.fill(0);
            self.marker = 1;
        }
        self.marker
    }

    /// True when every input vertex of `v`'s cluster lies within the tolerance of q.
    fn bound_ok(&self, v: u32, q: P3) -> bool {
        let vi = v as usize;
        if length(sub(q, self.center[vi])) + self.reach[vi] <= self.tolerance {
            return true;
        }
        let mut w = v;
        while w != NONE {
            if length(sub(q, self.input_at(w))) > self.tolerance {
                return false;
            }
            w = self.next[w as usize];
        }
        true
    }

    /// True when vertex `v` is frozen, or its triangles were found not to form one clean fan and
    /// have not changed since: no fix moves it.
    fn known_bad(&self, v: u32) -> bool {
        self.frozen[v as usize] || self.bad_ring[v as usize] == self.changed_at[v as usize]
    }

    /// Collects the fan round vertex `v` into `r` (see `walk`), remembering a failure until the
    /// vertex's triangles change.
    fn ring(&mut self, v: u32, r: &mut Ring) -> bool {
        if self.known_bad(v) {
            return false;
        }
        let fan = self.walk(v, r);
        if !fan {
            self.bad_ring[v as usize] = self.changed_at[v as usize];
        }
        fan
    }

    /// The fan round corner `k` (vertex `v`) of the triangle being fixed, in `sc.rings[k]`: walked
    /// once per attempt. False when it is not one clean fan.
    fn fan(&mut self, sc: &mut Scratch, k: usize, v: u32) -> bool {
        if sc.ring_of[k] != v {
            sc.ring_of[k] = v;
            sc.fan_ok[k] = self.ring(v, &mut sc.rings[k]);
            sc.measured[k] = false;
        }
        sc.fan_ok[k]
    }

    /// Measures the fan walked for corner `k` (see `fan`), once per attempt.
    fn measured(&self, sc: &mut Scratch, k: usize) {
        if !sc.measured[k] {
            self.measure(&mut sc.rings[k]);
            sc.measured[k] = true;
        }
    }

    /// Collects the sides leaving vertex `v` into `r`, one per triangle of its fan, in turning
    /// order. False when the vertex's triangles do not form one fan joined through clean edges: a
    /// non-manifold or inconsistently oriented spot, or several fans meeting at the vertex.
    fn walk(&self, v: u32, r: &mut Ring) -> bool {
        r.sides.clear();
        r.open = false;
        let first = self.out[v as usize];
        let valence = self.valence[v as usize] as usize;
        if first == NONE || valence == 0 {
            return false;
        }
        // Back round the fan to the triangle whose side leaving v is a border side, or all the way
        // round to where the walk began.
        let mut s = first;
        for _ in 0..valence {
            match self.twin[s as usize] {
                BORDER => break,
                BAD => return false,
                across => {
                    s = next(across);
                    if s == first {
                        break;
                    }
                }
            }
        }
        let start = s;
        loop {
            debug_assert_eq!(self.start(s), v);
            r.sides.push(s);
            if r.sides.len() > valence {
                return false;
            }
            match self.twin[prev(s) as usize] {
                BORDER => {
                    r.open = true;
                    break;
                }
                BAD => return false,
                across => {
                    if across == start {
                        break;
                    }
                    s = across;
                }
            }
        }
        r.sides.len() == valence
    }

    /// Measures the triangles of a fan `ring` collected (their shapes before the change) and the
    /// vertex's rank.
    fn measure(&self, r: &mut Ring) {
        r.shapes.clear();
        for &s in &r.sides {
            r.shapes.push(shape(self.corners(s / 3), self.tolerance));
        }
        let k = r.sides.len();
        let mut features = 0;
        if r.open {
            // The two border edges, and a corner when the border turns sharply here.
            features += 2;
            let v = self.pos[self.start(r.sides[0]) as usize];
            let d1 = sub(self.pos[self.end(r.sides[0]) as usize], v);
            let d2 = sub(self.pos[self.start(prev(r.sides[k - 1])) as usize], v);
            if dot(d1, d2) > -SHARP * length(d1) * length(d2) {
                features += 1;
            }
        }
        let pairs = if r.open { k - 1 } else { k };
        for i in 0..pairs {
            let (x, y) = (&r.shapes[i], &r.shapes[(i + 1) % k]);
            if x.trusted
                && y.trusted
                && dot(x.normal, y.normal) < SHARP * length(x.normal) * length(y.normal)
            {
                features += 1;
            }
        }
        r.rank = match features {
            0 => 0,
            1 | 2 => 1,
            _ => 2,
        };
    }

    /// True when `w` is one of the fan's neighbouring vertices.
    fn has_neighbor(&self, r: &Ring, w: u32) -> bool {
        r.sides.iter().any(|&s| self.end(s) == w)
            || (r.open && self.start(prev(r.sides[r.sides.len() - 1])) == w)
    }

    /// True when the fan has a triangle whose two other corners are `a` and `b`.
    fn spans(&self, r: &Ring, a: u32, b: u32) -> bool {
        r.sides.iter().any(|&s| {
            let (x, y) = (self.end(s), self.start(prev(s)));
            (x == a && y == b) || (x == b && y == a)
        })
    }

    /// The link condition for collapsing the edge of side `s` (u → v, fans `ru` and `rv`): the
    /// vertices next to both are the third corners of the one or two triangles on the edge, two
    /// border vertices are joined only along a border edge, and no part vanishes whole (a flap, a
    /// double-sided pair, a tetrahedron folding into two triangles on the same corners).
    fn link_ok(&mut self, s: u32, ru: &Ring, rv: &Ring) -> bool {
        let u = self.start(s);
        let a = self.end(next(s));
        let across = self.twin[s as usize];
        let b = if across == BORDER {
            NONE
        } else {
            self.end(next(across))
        };
        if across == BORDER {
            if self.twin[next(s) as usize] == BORDER && self.twin[prev(s) as usize] == BORDER {
                return false;
            }
        } else if a == b || (ru.open && rv.open) {
            return false;
        }
        let mark = self.new_mark();
        for &side in &ru.sides {
            let w = self.end(side);
            self.mark[w as usize] = mark;
        }
        if ru.open {
            let w = self.start(prev(ru.sides[ru.sides.len() - 1]));
            self.mark[w as usize] = mark;
        }
        let common = |w: u32| w != u && w != a && w != b && self.mark[w as usize] == mark;
        if rv.sides.iter().any(|&side| common(self.end(side)))
            || (rv.open && common(self.start(prev(rv.sides[rv.sides.len() - 1]))))
        {
            return false;
        }
        !(across != BORDER && self.spans(ru, a, b) && self.spans(rv, a, b))
    }

    /// Triangle `t`'s squared edge lengths (edge k joins corner k to corner k + 1) and its edges
    /// from shortest to longest.
    fn edges_of(&self, t: u32) -> ([f64; 3], [usize; 3]) {
        let p = self.corners(t);
        let lengths = [sub(p[1], p[0]), sub(p[2], p[1]), sub(p[0], p[2])].map(|d| dot(d, d));
        let mut order = [0usize, 1, 2];
        order.sort_by(|&i, &j| lengths[i].total_cmp(&lengths[j]));
        (lengths, order)
    }

    /// True when every fix `fix` would try on triangle `t` needs a vertex known to have no clean
    /// fan or a non-manifold edge: each short edge of a needle; for a cap, its far corner (which
    /// its split and short-edge collapses all need), or its long edge together with every
    /// short edge it could collapse instead.
    fn hopeless(&self, t: u32) -> bool {
        let c = self.tri[t as usize];
        let (lengths, order) = self.edges_of(t);
        let tolerance = self.tolerance;
        let below = tolerance - f32_slack(self.corners(t));
        let short = |k: usize| lengths[k].sqrt() < below || lengths[k] == 0.0;
        let blocked = |k: usize| {
            self.twin[t as usize * 3 + k] == BAD
                || self.known_bad(c[k])
                || self.known_bad(c[(k + 1) % 3])
        };
        if short(order[0]) {
            return order.iter().take_while(|&&k| short(k)).all(|&k| blocked(k));
        }
        let long = order[2];
        self.known_bad(c[(long + 2) % 3])
            || (self.twin[t as usize * 3 + long] == BAD
                && order[..2]
                    .iter()
                    .filter(|&&k| lengths[k].sqrt() <= 2.0 * tolerance)
                    .all(|&k| blocked(k)))
    }

    /// One attempt at triangle `t`.
    fn fix(&mut self, t: u32, sc: &mut Scratch) -> Fix {
        sc.ring_of = [NONE; 3];
        // A zero-length edge is short at any tolerance.
        let (lengths, order) = self.edges_of(t);
        let tolerance = self.tolerance;
        let below = tolerance - f32_slack(self.corners(t));
        let short = |k: usize| lengths[k].sqrt() < below || lengths[k] == 0.0;
        let mut result: Option<Fix> = None;
        if short(order[0]) {
            for &k in order.iter().take_while(|&&k| short(k)) {
                let r = self.try_collapse(t * 3 + k as u32, sc);
                if r == Fix::Done {
                    return r;
                }
                result = join(result, r);
            }
            return result.unwrap_or(Fix::Stuck);
        }
        let s = t * 3 + order[2] as u32;
        let r = self.try_split(s, sc);
        if r == Fix::Done {
            return r;
        }
        result = join(result, r);
        // Last, a short edge that collapses within the bound (twice the tolerance long at most):
        // the split leaves a sliver where the far corner lies near an end of the long edge, or
        // where the triangle across is itself barely thicker than the tolerance.
        for &k in &order[..2] {
            if lengths[k].sqrt() > 2.0 * tolerance {
                break;
            }
            let r = self.try_collapse(t * 3 + k as u32, sc);
            if r == Fix::Done {
                return r;
            }
            result = join(result, r);
        }
        result.unwrap_or(Fix::Stuck)
    }

    /// Collapses the edge of side `s` (u → v) when the displacement bound, the link condition and
    /// the shape checks allow it: into the endpoint on a border or feature line when only one lies
    /// on one, else to the midpoint, else into either endpoint; when all of those move some input
    /// vertex beyond the tolerance, to the point between the two clusters' bounding spheres.
    fn try_collapse(&mut self, s: u32, sc: &mut Scratch) -> Fix {
        let (u, v) = (self.start(s), self.end(s));
        if self.twin[s as usize] == BAD || self.known_bad(u) || self.known_bad(v) {
            return Fix::Tangled;
        }
        // u and v are input vertices of their own clusters: no point lies within the tolerance of
        // both when they started more than twice the tolerance apart.
        if length(sub(self.input_at(u), self.input_at(v))) > 2.0 * self.tolerance {
            return Fix::Spent;
        }
        // The fans of u and v are those of the triangle's corners s % 3 and the one after.
        let (ku, kv) = ((s % 3) as usize, ((s + 1) % 3) as usize);
        if !self.fan(sc, ku, u) || !self.fan(sc, kv, v) {
            return Fix::Tangled;
        }
        if !self.link_ok(s, &sc.rings[ku], &sc.rings[kv]) {
            return Fix::Stuck;
        }
        self.measured(sc, ku);
        self.measured(sc, kv);
        let choices: &[f64] = match sc.rings[ku].rank.cmp(&sc.rings[kv].rank) {
            Ordering::Greater => &[0.0, 0.5],
            Ordering::Less => &[1.0, 0.5],
            Ordering::Equal => &[0.5, 0.0, 1.0],
        };
        let (pu, pv) = (self.pos[u as usize], self.pos[v as usize]);
        let mut tried = 0;
        let mut held = 0;
        for &f in choices {
            let q = round32(lerp(pu, pv, f));
            // A point farther than the tolerance from where u or v is now is out of reach in any
            // run; only the bound on what moved before can change with a new run.
            if length(sub(q, pu)) > self.tolerance || length(sub(q, pv)) > self.tolerance {
                continue;
            }
            tried += 1;
            if !self.bound_ok(u, q) || !self.bound_ok(v, q) {
                held += 1;
            } else if self.collapse_ok(u, v, q, &sc.rings[ku], &sc.rings[kv], &mut sc.check) {
                self.apply_collapse(s, q, sc);
                return Fix::Done;
            }
        }
        if held < tried || held == 0 {
            return if held > 0 { Fix::Held } else { Fix::Stuck };
        }
        let (ui, vi) = (u as usize, v as usize);
        let reach = |q: P3| {
            (length(sub(q, self.center[ui])) + self.reach[ui])
                .max(length(sub(q, self.center[vi])) + self.reach[vi])
        };
        let q = round32(lerp(pu, pv, argmin(|f| reach(lerp(pu, pv, f)))));
        if !self.bound_ok(u, q) || !self.bound_ok(v, q) {
            return Fix::Spent;
        }
        if self.collapse_ok(u, v, q, &sc.rings[ku], &sc.rings[kv], &mut sc.check) {
            self.apply_collapse(s, q, sc);
            return Fix::Done;
        }
        Fix::Held
    }

    /// True when moving u and v together to q keeps every remaining triangle round them good and
    /// folds nothing over (see `folds`).
    fn collapse_ok(&self, u: u32, v: u32, q: P3, ru: &Ring, rv: &Ring, check: &mut Check) -> bool {
        // The neighbourhood's normal: the area-weighted sum of the remaining triangles' normals.
        let mut reference = [0.0; 3];
        for (r, other) in [(ru, v), (rv, u)] {
            for (i, &side) in r.sides.iter().enumerate() {
                if !self.tri[(side / 3) as usize].contains(&other) {
                    reference = add(reference, r.shapes[i].normal);
                }
            }
        }
        check.clear();
        for (r, me, other) in [(ru, u, v), (rv, v, u)] {
            for (i, &side) in r.sides.iter().enumerate() {
                let c = self.tri[(side / 3) as usize];
                if c.contains(&other) {
                    continue;
                }
                let corners = c.map(|w| if w == me { q } else { self.pos[w as usize] });
                let new = shape(corners, self.tolerance);
                if !keeps(&r.shapes[i], &new, reference) {
                    return false;
                }
                // Its outer edge is the side after the one leaving `me`.
                self.note_beside(next(side), check.made.len(), check);
                check.made.push((corners, new));
            }
        }
        !check.folds()
    }

    /// Notes the triangle across side `s`, if any, as lying beside made triangle `i`.
    fn note_beside(&self, s: u32, i: usize, check: &mut Check) {
        let across = self.twin[s as usize];
        if across < BAD {
            let p = self.corners(across / 3);
            check.beside.push((i, p, shape(p, self.tolerance)));
        }
    }

    /// Collapses the edge of side `s` (u → v) to q: the one or two triangles on it go, the sides
    /// across each one's other two edges become twins, and v's triangles take u instead. The fans
    /// of u and v are the walked ones of the triangle's corners s % 3 and the one after.
    fn apply_collapse(&mut self, s: u32, q: P3, sc: &mut Scratch) {
        let (u, v) = (self.start(s), self.end(s));
        let (ku, kv) = ((s % 3) as usize, ((s + 1) % 3) as usize);
        let mut dying = 0;
        for side in [s, self.twin[s as usize]] {
            if side == BORDER {
                continue;
            }
            // The triangle (from, to, w) on the edge: its sides to → w and w → from close up.
            let (n, p) = (next(side), prev(side));
            let (x, y) = (self.twin[n as usize], self.twin[p as usize]);
            let w = self.end(n);
            self.glue(x, y);
            self.kill(side / 3);
            self.valence[w as usize] -= 1;
            // x leaves w; when it is a border side, the side after y does.
            self.out[w as usize] = if x != BORDER { x } else { next(y) };
            dying += 1;
        }
        sc.changed.clear();
        for &side in &sc.rings[kv].sides {
            let t = side / 3;
            if self.alive(t) {
                self.tri[t as usize][(side % 3) as usize] = u;
                sc.changed.push(t);
            }
        }
        for &side in &sc.rings[ku].sides {
            if self.alive(side / 3) {
                sc.changed.push(side / 3);
            }
        }
        let (ui, vi) = (u as usize, v as usize);
        self.pos[ui] = q;
        self.valence[ui] = self.valence[ui] + self.valence[vi] - 2 * dying;
        self.valence[vi] = 0;
        self.out[vi] = NONE;
        let t = sc.changed[0];
        let k = self.tri[t as usize]
            .iter()
            .position(|&w| w == u)
            .unwrap_or(0);
        self.out[ui] = t * 3 + k as u32;
        (self.center[ui], self.reach[ui]) = enclose(
            self.center[ui],
            self.reach[ui],
            self.center[vi],
            self.reach[vi],
        );
        self.next[self.tail[ui] as usize] = v;
        self.tail[ui] = self.tail[vi];
    }

    /// Moves the cap's far corner c onto its long edge (side `s`, a → b) at its projection, which
    /// removes the cap and splits the triangle across that edge (which then has c as a corner),
    /// when the link condition, the shape checks and the displacement bound allow it. When the
    /// projection moves an input vertex of c's cluster beyond the tolerance, the point of the edge
    /// nearest the cluster's bounding sphere is tried as well.
    fn try_split(&mut self, s: u32, sc: &mut Scratch) -> Fix {
        let (a, b, c) = (self.start(s), self.end(s), self.end(next(s)));
        let across = self.twin[s as usize];
        let kc = ((s + 2) % 3) as usize;
        if across == BAD || !self.fan(sc, kc, c) {
            return Fix::Tangled;
        }
        let d = if across == BORDER {
            NONE
        } else {
            self.end(next(across))
        };
        // A border corner moved onto another border would pinch the surface; an edge c-d already
        // there would get a third triangle.
        if (across == BORDER && sc.rings[kc].open)
            || (across != BORDER && (d == c || self.has_neighbor(&sc.rings[kc], d)))
        {
            return Fix::Stuck;
        }
        let [pa, pb, pc] = [a, b, c].map(|w| self.pos[w as usize]);
        let ab = sub(pb, pa);
        let onto = |p: P3| {
            round32(lerp(
                pa,
                pb,
                (dot(sub(p, pa), ab) / dot(ab, ab)).clamp(0.0, 1.0),
            ))
        };
        let projection = if collinear(pa, pb, pc) { pc } else { onto(pc) };
        let mut tried = 0;
        let mut held = 0;
        for m in [projection, onto(self.center[c as usize])] {
            if m == pa
                || m == pb
                || (tried > 0 && m == projection)
                || length(sub(m, pc)) > self.tolerance
            {
                continue;
            }
            tried += 1;
            // Moving c leaves its cluster's input positions, and so its bounding sphere, as they
            // were.
            if !self.bound_ok(c, m) {
                held += 1;
                continue;
            }
            self.measured(sc, kc);
            match self.split_ok(s, m, &sc.rings[kc], &mut sc.check) {
                Some(false) => {}
                Some(true) if self.stalls < self.stall_limit => self.stalls += 1,
                Some(true) => {
                    held += 1;
                    continue;
                }
                None => continue,
            }
            self.apply_split(s, m, sc);
            return Fix::Done;
        }
        if tried > 0 && held == tried {
            Fix::Spent
        } else if held > 0 {
            Fix::Held
        } else {
            Fix::Stuck
        }
    }

    /// Whether moving the cap's far corner c (on side `s`, a → b; fan `r`, measured) to m keeps
    /// every triangle good and folds nothing over: None when not, else whether the split leaves as
    /// many degenerate triangles as before (a stall).
    fn split_ok(&self, s: u32, m: P3, r: &Ring, check: &mut Check) -> Option<bool> {
        let t = s / 3;
        let (a, b, c) = (self.start(s), self.end(s), self.end(next(s)));
        let across = self.twin[s as usize];
        let before = (across != BORDER).then(|| shape(self.corners(across / 3), self.tolerance));
        let mut reference = before.map_or([0.0; 3], |b| b.normal);
        for (i, &side) in r.sides.iter().enumerate() {
            if side / 3 != t {
                reference = add(reference, r.shapes[i].normal);
            }
        }
        check.clear();
        // Degenerate triangles fixed minus made: the cap itself goes.
        let mut delta = -1i64;
        for (i, &side) in r.sides.iter().enumerate() {
            let f = side / 3;
            if f == t {
                continue;
            }
            let corners =
                self.tri[f as usize].map(|w| if w == c { m } else { self.pos[w as usize] });
            let new = shape(corners, self.tolerance);
            if !keeps(&r.shapes[i], &new, reference) {
                return None;
            }
            delta -= i64::from(r.shapes[i].degenerate && !new.degenerate);
            self.note_beside(next(side), check.made.len(), check);
            check.made.push((corners, new));
        }
        let stall = match before {
            None => false,
            Some(old) => {
                let [pa, pb] = [a, b].map(|w| self.pos[w as usize]);
                let pd = self.pos[self.end(next(across)) as usize];
                // The halves' outer edges are the triangle across's sides d → b and a → d.
                for (corners, outer) in [([pb, m, pd], prev(across)), ([m, pa, pd], next(across))] {
                    let new = shape(corners, self.tolerance);
                    if !keeps(&old, &new, reference) {
                        return None;
                    }
                    delta += i64::from(new.degenerate);
                    self.note_beside(outer, check.made.len(), check);
                    check.made.push((corners, new));
                }
                delta -= i64::from(old.degenerate);
                // Only a thin quad, whose triangle across is a cap over the same edge, needs a step
                // that leaves as many degenerate triangles as before: its halves are caps over the
                // edge's two parts, which the next steps fix. Anything else would only churn.
                let [ab, ad, bd] = [sub(pb, pa), sub(pd, pa), sub(pd, pb)].map(|e| dot(e, e));
                if delta >= 0 && (ab < ad || ab < bd || ad.min(bd).sqrt() < self.tolerance) {
                    return None;
                }
                delta >= 0
            }
        };
        (!check.folds()).then_some(stall)
    }

    /// Moves c (the cap on side `s`, a → b) to m on its long edge: the cap goes, and the triangle
    /// across, (b, a, d), becomes (b, c, d) plus a new (c, a, d) at the end. The fan of c is the
    /// walked one of the triangle's corner after the edge.
    fn apply_split(&mut self, s: u32, m: P3, sc: &mut Scratch) {
        let t = s / 3;
        let (a, b, c) = (self.start(s), self.end(s), self.end(next(s)));
        // The sides across the cap's two short edges: x runs c → b, y runs a → c.
        let x = self.twin[next(s) as usize];
        let y = self.twin[prev(s) as usize];
        let across = self.twin[s as usize];
        sc.changed.clear();
        for &side in &sc.rings[((s + 2) % 3) as usize].sides {
            if side / 3 != t {
                sc.changed.push(side / 3);
            }
        }
        self.kill(t);
        for w in [a, b, c] {
            self.valence[w as usize] -= 1;
        }
        self.pos[c as usize] = m;
        if across == BORDER {
            // c was inside (try_split checks), so both short edges had a triangle across.
            self.glue(x, BORDER);
            self.glue(y, BORDER);
            self.out[a as usize] = y;
            self.out[b as usize] = next(x);
            self.out[c as usize] = x;
        } else {
            let t2 = across / 3;
            let d = self.end(next(across));
            let ad = self.twin[next(across) as usize];
            let db = self.twin[prev(across) as usize];
            self.tri[t2 as usize] = [b, c, d];
            let t3 = self.push_triangle([c, a, d], self.origin[t2 as usize]);
            let (f, g) = (t2 * 3, t3 * 3);
            self.glue(f, x);
            self.glue(f + 1, g + 2);
            self.glue(f + 2, db);
            self.glue(g, y);
            self.glue(g + 1, ad);
            // (b, a, d) lost a and gained c; (c, a, d) is new.
            self.valence[c as usize] += 2;
            self.valence[d as usize] += 1;
            self.out[a as usize] = g + 1;
            self.out[b as usize] = f;
            self.out[c as usize] = f + 1;
            self.out[d as usize] = f + 2;
            sc.changed.extend([t2, t3]);
        }
    }

    /// After a fix: stamp the corners of the changed triangles with the new epoch, and queue the
    /// changed triangles that are degenerate.
    fn after_change(&mut self, sc: &mut Scratch) {
        self.epoch += 1;
        for &t in &sc.changed {
            self.touched[t as usize] = true;
            for v in self.tri[t as usize] {
                self.changed_at[v as usize] = self.epoch;
            }
        }
        for i in 0..sc.changed.len() {
            self.rekey(sc.changed[i]);
        }
    }

    /// Runs the queue to its end in rounds: the queue runs dry, then the failed triangles that a
    /// fix has changed something round since they were tried go in again, until a round fixes
    /// nothing.
    fn settle(&mut self, control: &mut Control) -> RepairResult<()> {
        let mut sc = Scratch::default();
        let mut steps = 0usize;
        let mut shown = 0.0f64;
        loop {
            while let Some((t, stamp)) = self.queue.pop() {
                steps += 1;
                if steps.is_multiple_of(CHECK_EVERY) {
                    control.check()?;
                    shown = shown.max(steps as f64 / (steps + self.queue.len) as f64);
                    control.progress(shown);
                }
                let ti = t as usize;
                if self.stamp[ti] != stamp || self.state[ti] != QUEUED || !self.alive(t) {
                    continue;
                }
                // A triangle that got thicker while it waited goes back to the band it belongs in.
                let band = self
                    .queue
                    .band(shape(self.corners(t), self.tolerance).height);
                if band > self.band[ti] {
                    self.band[ti] = band;
                    self.queue.push(band, t, stamp);
                    continue;
                }
                self.state[ti] = IDLE;
                match self.fix(t, &mut sc) {
                    Fix::Done => {
                        self.changed = true;
                        self.after_change(&mut sc);
                    }
                    fix => self.failed(t, fix),
                }
            }
            if !self.requeue() {
                return Ok(());
            }
        }
    }

    /// The input vertices of the triangles a fix changed or made that cross another triangle
    /// (tritri's rule, exact), where the surface folded over itself or was pushed into a nearby
    /// part, and how many such triangles there are: those vertices stay put in the next attempt.
    /// A hierarchy over the changed triangles finds the candidates, every live triangle tested
    /// against it in parallel. A crossing where either triangle comes from an input triangle that
    /// already crossed the input surface continues damage the input had and does not count.
    fn crossings(&self, mesh: &Mesh, control: &Control) -> RepairResult<(Vec<u32>, usize)> {
        let changed: Vec<u32> = (0..self.tri.len() as u32)
            .filter(|&t| self.alive(t) && self.touched[t as usize])
            .collect();
        if changed.is_empty() {
            return Ok((Vec::new(), 0));
        }
        let box_of = |p: [P3; 3]| {
            let mut b = EMPTY;
            for q in p {
                include(&mut b, q);
            }
            b
        };
        let boxes: Vec<Aabb> = changed.iter().map(|&t| box_of(self.corners(t))).collect();
        let bvh = Bvh::new(&boxes);
        let cancel = control.cancel_flag();
        let live: Vec<u32> = (0..self.tri.len() as u32)
            .filter(|&t| self.alive(t))
            .collect();
        let pairs: Vec<(u32, u32)> = live
            .par_iter()
            .flat_map_iter(|&g| {
                let mut found = Vec::new();
                if cancel.load(AtomicOrdering::Relaxed) {
                    return found;
                }
                let pg = self.corners(g);
                let cg = self.tri[g as usize];
                bvh.query(&box_of(pg), |i| {
                    let t = changed[i as usize];
                    let pt = self.corners(t);
                    if t != g && !hinged(self.tri[t as usize], cg, &pt, &pg) && intersect(&pt, &pg)
                    {
                        found.push((t, g));
                    }
                });
                found
            })
            .collect();
        control.check()?;
        // The input triangles the crossing ones come from, and which of those crossed the input
        // surface already: a crossing where either side comes from such damage continues it.
        let mut sources: Vec<u32> = pairs
            .iter()
            .flat_map(|&(t, g)| [self.origin[t as usize], self.origin[g as usize]])
            .collect();
        sources.sort_unstable();
        sources.dedup();
        let mut damaged = vec![false; sources.len()];
        if !sources.is_empty() {
            // A hierarchy over the sources, every input triangle tested against it in parallel.
            let boxes: Vec<Aabb> = sources
                .iter()
                .map(|&o| box_of(input_corners(mesh, o)))
                .collect();
            let near = Bvh::new(&boxes);
            let crossed: Vec<usize> = (0..mesh.triangle_count() as u32)
                .into_par_iter()
                .flat_map_iter(|h| {
                    let mut found = Vec::new();
                    let ph = input_corners(mesh, h);
                    let ch = input_triangle(mesh, h);
                    near.query(&box_of(ph), |i| {
                        let o = sources[i as usize];
                        let po = input_corners(mesh, o);
                        if o != h
                            && !hinged(input_triangle(mesh, o), ch, &po, &ph)
                            && intersect(&po, &ph)
                        {
                            found.push(i as usize);
                        }
                    });
                    found
                })
                .collect();
            for i in crossed {
                damaged[i] = true;
            }
        }
        let new = |t: u32| {
            sources
                .binary_search(&self.origin[t as usize])
                .is_ok_and(|i| !damaged[i])
        };
        let mut hits: Vec<u32> = pairs
            .iter()
            .filter(|&&(t, g)| new(t) && new(g))
            .map(|&(t, _)| t)
            .collect();
        hits.sort_unstable();
        hits.dedup();
        // Every input vertex merged into a corner of a crossing triangle.
        let crossed = hits.len();
        let mut vertices = Vec::new();
        for t in hits {
            for v in self.tri[t as usize] {
                let mut w = v;
                while w != NONE {
                    vertices.push(w);
                    w = self.next[w as usize];
                }
            }
        }
        vertices.sort_unstable();
        vertices.dedup();
        Ok((vertices, crossed))
    }

    /// After a failed attempt at triangle `t`: remember the reason, and list the triangle for the
    /// next round unless only the displacement bound held it back.
    fn failed(&mut self, t: u32, fix: Fix) {
        let ti = t as usize;
        self.state[ti] = match fix {
            Fix::Spent => SPENT,
            Fix::Held => HELD,
            Fix::Tangled => TANGLED,
            _ => STUCK,
        };
        self.tried_at[ti] = self.epoch;
        if fix != Fix::Spent && !self.listed[ti] {
            self.listed[ti] = true;
            self.failed.push(t);
        }
    }

    /// True while triangle `t` touches a vertex whose fan is known to be bad or an edge that is
    /// non-manifold or inconsistently oriented, which is what holds a tangled triangle back.
    fn tangled(&self, t: u32) -> bool {
        self.tri[t as usize].iter().any(|&v| self.known_bad(v))
            || (0..3).any(|k| self.twin[(t * 3 + k) as usize] == BAD)
    }

    /// Queues triangle `t` afresh when it may be fixed and is degenerate. A triangle still queued
    /// in the same band or a thinner one keeps its place: when that comes up, it goes back to the
    /// band it belongs in.
    fn rekey(&mut self, t: u32) {
        let ti = t as usize;
        let height = (self.alive(t) && self.allowed(t))
            .then(|| shape(self.corners(t), self.tolerance))
            .and_then(|s| s.degenerate.then_some(s.height));
        match height {
            Some(h) => {
                let band = self.queue.band(h);
                if self.state[ti] == QUEUED && self.band[ti] <= band {
                    return;
                }
                self.stamp[ti] = self.stamp[ti].wrapping_add(1);
                self.state[ti] = QUEUED;
                self.band[ti] = band;
                self.queue.push(band, t, self.stamp[ti]);
            }
            None => {
                self.stamp[ti] = self.stamp[ti].wrapping_add(1);
                self.state[ti] = IDLE;
            }
        }
    }

    /// Queues for the next round the listed triangles a fix changed something round since they
    /// were tried (for a tangled one, only once nothing holds it back any more). False when none
    /// went in.
    fn requeue(&mut self) -> bool {
        let mut failed = std::mem::take(&mut self.failed);
        let mut any = false;
        failed.retain(|&t| {
            let ti = t as usize;
            let state = self.state[ti];
            if !self.alive(t) || !matches!(state, STUCK | HELD | TANGLED) {
                self.listed[ti] = false;
                return false;
            }
            let tried = self.tried_at[ti];
            let moved = self.tri[ti]
                .iter()
                .any(|&v| self.changed_at[v as usize] > tried);
            if !moved || (state == TANGLED && self.tangled(t)) {
                return true;
            }
            self.listed[ti] = false;
            self.rekey(t);
            any |= self.state[ti] == QUEUED;
            false
        });
        self.failed = failed;
        any
    }

    /// Checks what the edits must keep (debug builds): each vertex's count of live triangles, a
    /// live side leaving it, and twins that pair up on one edge running opposite ways.
    fn audit(&self) {
        let mut valence = vec![0u32; self.pos.len()];
        for (t, c) in self.tri.iter().enumerate() {
            if c[0] == DEAD {
                continue;
            }
            for k in 0..3 {
                let s = (t * 3 + k) as u32;
                valence[c[k] as usize] += 1;
                let across = self.twin[s as usize];
                if across < BAD {
                    assert!(
                        self.alive(across / 3),
                        "side {s} is twinned with a removed triangle"
                    );
                    assert_eq!(
                        self.twin[across as usize], s,
                        "side {s}'s twin is not mutual"
                    );
                    assert_eq!(
                        (self.start(across), self.end(across)),
                        (self.end(s), self.start(s)),
                        "side {s} and its twin do not run opposite ways on one edge"
                    );
                }
            }
        }
        for (v, &n) in valence.iter().enumerate() {
            assert_eq!(self.valence[v], n, "vertex {v}'s triangle count");
            let s = self.out[v];
            if n > 0 {
                assert!(
                    s != NONE && self.alive(s / 3) && self.start(s) == v as u32,
                    "vertex {v}"
                );
            }
        }
    }

    /// The edited mesh: live triangles in order (a split's new half last), the vertices they use
    /// in their order; and what is left degenerate, by reason.
    fn finish(self, mesh: &Mesh, found: usize) -> Work {
        let mut bound = 0;
        let mut tangled = 0;
        let mut stuck = 0;
        let mut crossing = 0;
        for (t, c) in self.tri.iter().enumerate() {
            let t = t as u32;
            if c[0] == DEAD
                || !self.allowed(t)
                || !(repeated(*c) || shape(self.corners(t), self.tolerance).degenerate)
            {
                continue;
            }
            if c.iter().any(|&v| self.frozen[v as usize]) {
                crossing += 1;
                continue;
            }
            match self.state[t as usize] {
                SPENT | HELD => bound += 1,
                TANGLED => tangled += 1,
                _ => stuck += 1,
            }
        }
        if !self.changed {
            return Work {
                mesh: mesh.clone(),
                origin: Origin::Same,
                changed: false,
                found,
                removed: 0,
                bound,
                tangled,
                stuck,
                crossing,
                #[cfg(test)]
                vertex_of: (0..mesh.vertex_count() as u32).collect(),
            };
        }
        let mut id = vec![NONE; self.pos.len()];
        for c in self.tri.iter().filter(|c| c[0] != DEAD) {
            for &v in c {
                id[v as usize] = 0;
            }
        }
        let mut positions = Vec::new();
        let mut next = 0u32;
        for (v, slot) in id.iter_mut().enumerate() {
            if *slot != NONE {
                *slot = next;
                next += 1;
                positions.extend(self.pos[v].map(|x| x as f32));
            }
        }
        let mut indices = Vec::with_capacity(self.live * 3);
        let mut origin = Vec::with_capacity(self.live);
        for (t, c) in self.tri.iter().enumerate() {
            if c[0] != DEAD {
                indices.extend(c.map(|v| id[v as usize]));
                origin.push(self.origin[t]);
            }
        }
        // Each output vertex's cluster lists the input vertices that ended up in it.
        #[cfg(test)]
        let vertex_of = {
            let mut of = vec![NONE; mesh.vertex_count()];
            for (u, &slot) in id.iter().enumerate().filter(|&(_, &slot)| slot != NONE) {
                let mut w = u as u32;
                while w != NONE {
                    of[w as usize] = slot;
                    w = self.next[w as usize];
                }
            }
            of
        };
        let same = origin.len() == mesh.triangle_count()
            && origin.iter().enumerate().all(|(i, &o)| o as usize == i);
        Work {
            mesh: Mesh { positions, indices },
            removed: mesh.triangle_count().saturating_sub(origin.len()),
            origin: if same {
                Origin::Same
            } else {
                Origin::Map(origin)
            },
            changed: true,
            found,
            bound,
            tangled,
            stuck,
            crossing,
            #[cfg(test)]
            vertex_of,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    use super::*;
    use crate::mesh::{make_box, stats};
    use crate::repair::defaults::DEGENERATE;
    use crate::repair::exact::testing::{Rng, torus};
    use crate::repair::exact::vertex;
    use crate::repair::status::{analyze, degenerate_triangles};
    use crate::repair::topology::bbox_diagonal;
    use crate::repair::{RepairError, remap_selection, silent, tritri};

    fn cube() -> Mesh {
        make_box([0.0; 3], [20.0; 3])
    }

    fn mesh(positions: &[f32], indices: &[u32]) -> Mesh {
        Mesh {
            positions: positions.to_vec(),
            indices: indices.to_vec(),
        }
    }

    fn default_tolerance(m: &Mesh) -> f64 {
        let _ = m;
        effective_degenerate_tolerance(&DEGENERATE)
    }

    fn remove(m: &Mesh) -> Outcome {
        silent(|c| run(m, &[], &DEGENERATE, c)).unwrap()
    }

    fn pass(m: &Mesh, tolerance: f64) -> Work {
        silent(|c| work(m, tolerance, None, c)).unwrap()
    }

    fn self_intersections(m: &Mesh) -> usize {
        silent(|c| tritri::intersecting_pairs(m, c)).unwrap().len()
    }

    /// Edges, border edges, non-manifold edges and the Euler characteristic V − E + F over the
    /// vertices triangles use.
    fn topology(m: &Mesh) -> (usize, usize, usize, i64) {
        let edges = Edges::new(m);
        let mut border = 0;
        let mut non_manifold = 0;
        for e in 0..edges.len() as u32 {
            match edges.degree(e) {
                1 => border += 1,
                2 => {}
                _ => non_manifold += 1,
            }
        }
        let mut used = vec![false; m.vertex_count()];
        for &i in &m.indices {
            used[i as usize] = true;
        }
        let v = used.iter().filter(|&&u| u).count() as i64;
        let euler = v - edges.len() as i64 + m.triangle_count() as i64;
        (edges.len(), border, non_manifold, euler)
    }

    /// Edges with two sides that run the same way (a triangle turned against its neighbour).
    fn misoriented(m: &Mesh) -> usize {
        let edges = Edges::new(m);
        (0..edges.len() as u32)
            .filter(|&e| match *edges.sides(e) {
                [s, r] => m.indices[s as usize] == m.indices[r as usize],
                _ => false,
            })
            .count()
    }

    /// What every pass must keep: no new border or non-manifold edges, the same Euler
    /// characteristic on a closed surface, no new inverted triangles, holes or self-intersections,
    /// every input vertex within the tolerance of where it ended up, and the volume within what
    /// moving the surface by the tolerance can change.
    fn assert_sound(before: &Mesh, w: &Work, tolerance: f64) {
        let after = &w.mesh;
        let (_, b0, n0, e0) = topology(before);
        let (_, b1, n1, e1) = topology(after);
        assert!(b1 <= b0, "border edges {b0} -> {b1}");
        assert!(n1 <= n0, "non-manifold edges {n0} -> {n1}");
        if b0 == 0 && n0 == 0 {
            assert_eq!(
                (b1, n1, e1),
                (0, 0, e0),
                "a closed surface changed topology"
            );
        }
        let s0 = silent(|c| analyze(before, c)).unwrap().status;
        let s1 = silent(|c| analyze(after, c)).unwrap().status;
        assert!(
            s1.inverted_triangles <= s0.inverted_triangles,
            "{s0:?} -> {s1:?}"
        );
        assert!(
            misoriented(after) <= misoriented(before),
            "a triangle turned"
        );
        assert!(s1.holes <= s0.holes, "{s0:?} -> {s1:?}");
        assert!(
            self_intersections(after) <= self_intersections(before),
            "new self-intersections"
        );
        for (v, &to) in w.vertex_of.iter().enumerate() {
            if to == NONE {
                continue;
            }
            let d = length(sub(vertex(before, v as u32), vertex(after, to)));
            assert!(
                d <= tolerance * (1.0 + 1e-9),
                "vertex {v} moved {d} > {tolerance}"
            );
        }
        let area = s0.area.max(1e-12);
        assert!(
            (s1.volume - s0.volume).abs() <= area * tolerance * 1.01 + 1e-9,
            "volume {} -> {}",
            s0.volume,
            s1.volume
        );
    }

    /// The cube with its top face (z = 20) replaced by a fan of four round a point `h` from its
    /// front edge: the triangle (4, 5, 8) that thin; at 0 a cap without area, as the cube-sliver
    /// fixture.
    fn cube_sliver(h: f32) -> Mesh {
        let mut m = cube();
        m.indices.drain(6..12);
        m.positions.extend_from_slice(&[10.0, h, 20.0]);
        for [a, b] in [[4, 5], [5, 6], [6, 7], [7, 4]] {
            m.indices.extend_from_slice(&[a, b, 8]);
        }
        m
    }

    #[test]
    fn the_degenerate_rule_is_the_status_rule() {
        let mut rng = Rng(3);
        for round in 0..200 {
            let mut positions = Vec::new();
            for _ in 0..30 {
                let base = [0, 1, 2].map(|_| (rng.unit() * 40.0 - 20.0) as f32);
                positions.extend_from_slice(&base);
            }
            let mut m = mesh(&positions, &[]);
            for _ in 0..60 {
                let a = rng.below(30) as u32;
                let b = rng.below(30) as u32;
                // Third corners near the edge, near a corner, on the edge, or anywhere.
                let (pa, pb) = (vertex(&m, a), vertex(&m, b));
                let s = rng.unit();
                let k = rng.below(4);
                let off = [0.0005, 0.002, 0.0, 3.0][k as usize] * rng.unit();
                let p = add(lerp(pa, pb, s), [off, -off, off * 0.5]);
                m.positions.extend(p.map(|x| x as f32));
                let c = m.vertex_count() as u32 - 1;
                m.indices.extend_from_slice(&[a, b, c]);
            }
            for tolerance in [0.0, 0.0007, 0.002, 0.01] {
                let ours: Vec<u32> = (0..m.triangle_count() as u32)
                    .filter(|&t| {
                        let c = [0, 1, 2].map(|k| m.indices[t as usize * 3 + k]);
                        repeated(c) || shape(c.map(|v| vertex(&m, v)), tolerance).degenerate
                    })
                    .collect();
                assert_eq!(ours, degenerate_triangles(&m, tolerance), "round {round}");
            }
        }
    }

    #[test]
    fn a_clean_cube_is_unchanged() {
        let m = cube();
        let out = remove(&m);
        assert!(!out.changed);
        assert_eq!(out.origin, Origin::Same);
        assert_eq!(out.mesh, m);
        assert_eq!(out.message, "No degenerate faces to remove.");
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn the_cube_sliver_splits_the_triangle_across() {
        let m = cube_sliver(0.0);
        let tolerance = default_tolerance(&m);
        assert_eq!(degenerate_triangles(&m, tolerance), vec![10]);
        let out = remove(&m);
        assert!(out.changed);
        assert_eq!(out.message, "Removed 1 degenerate face.");
        assert!(out.warnings.is_empty());
        let s = silent(|c| analyze(&out.mesh, c)).unwrap().status;
        assert_eq!(s.degenerate_triangles, 0, "{s:?}");
        assert!(s.closed && s.oriented, "{s:?}");
        assert_eq!(s.volume, 8000.0);
        // The cap went and the front triangle across its long edge split in two at the apex (the
        // new half appended).
        assert_eq!(out.mesh.triangle_count(), 14);
        assert!(
            out.mesh
                .positions
                .as_chunks::<3>()
                .0
                .contains(&[10.0, 0.0, 20.0])
        );
        let Origin::Map(map) = &out.origin else {
            panic!("triangles were removed and added")
        };
        assert!(!map.contains(&10));
        assert_eq!(map.len(), 14);
        // The front face's triangle (0, 5, 4) is triangle 3: both halves map to it.
        assert_eq!(map.iter().filter(|&&o| o == 3).count(), 2);
        assert_sound(&m, &pass(&m, tolerance), tolerance);
        // A second run finds nothing.
        let again = remove(&out.mesh);
        assert!(!again.changed, "{}", again.message);
        assert_eq!(again.message, "No degenerate faces to remove.");
    }

    #[test]
    fn a_cap_inside_a_flat_face_splits_the_triangle_across() {
        // The top face: (4, 5, c), (5, 6, c), the cap (6, 4, c) with c on the diagonal 6-4, and
        // (4, 6, 7) across the diagonal.
        let mut m = cube();
        m.positions.extend_from_slice(&[10.0, 10.0, 20.0]);
        m.indices[6..12].copy_from_slice(&[4, 5, 8, 5, 6, 8]);
        m.indices.extend_from_slice(&[6, 4, 8, 4, 6, 7]);
        let tolerance = default_tolerance(&m);
        assert_eq!(degenerate_triangles(&m, tolerance), vec![12]);
        let out = remove(&m);
        assert_eq!(out.message, "Removed 1 degenerate face.");
        // Nothing moves; (4, 6, 7) became two halves round c, the cap went.
        assert_eq!(out.mesh.positions, m.positions);
        assert_eq!(out.mesh.triangle_count(), 14);
        let s = silent(|c| analyze(&out.mesh, c)).unwrap().status;
        assert!(
            s.closed && s.oriented && s.degenerate_triangles == 0,
            "{s:?}"
        );
        assert_eq!(s.volume, 8000.0);
        assert!(!remove(&out.mesh).changed);
    }

    /// An open patch: the cap (a, b, c) with c on its long edge a-b, closed round c by
    /// (b, e, c), (e, f, c), (f, a, c), and across a-b the triangle (b, a, d), tilted by `tilt`
    /// degrees out of the cap's plane.
    fn cap_over_a_crease(tilt: f64) -> Mesh {
        let (s, c) = tilt.to_radians().sin_cos();
        let d = [5.0, -5.0 * c, 5.0 * s].map(|x| x as f32);
        mesh(
            &[
                0.0, 0.0, 0.0, // a
                10.0, 0.0, 0.0, // b
                5.0, 0.0, 0.0, // c
                d[0], d[1], d[2], // d
                10.0, 5.0, 0.0, // e
                0.0, 5.0, 0.0, // f
            ],
            &[0, 1, 2, 1, 4, 2, 4, 5, 2, 5, 0, 2, 1, 0, 3],
        )
    }

    #[test]
    fn a_cap_splits_the_triangle_across_at_any_crease() {
        let tolerance = 0.001;
        for tilt in [0.0, 9.5, 15.0, 90.0] {
            let m = cap_over_a_crease(tilt);
            assert_eq!(degenerate_triangles(&m, tolerance), vec![0]);
            let w = pass(&m, tolerance);
            assert_sound(&m, &w, tolerance);
            // The cap went and (b, a, d) became two at c, which stays where it was.
            assert_eq!(w.mesh.vertex(w.vertex_of[2]), [5.0, 0.0, 0.0], "{tilt}°");
            assert_eq!(w.mesh.triangle_count(), 5);
            assert!(degenerate_triangles(&w.mesh, tolerance).is_empty());
        }
    }

    #[test]
    fn a_cap_near_the_end_of_its_long_edge_collapses_its_short_edge() {
        // The cap (a, b, c) has c on its long edge a-b, 0.014 from b; across a-b, (b, a, d) is
        // tilted 20 degrees with d 0.0085 from c. Splitting at c would make c-d a needle, so the
        // cap's 0.014 edge b-c (under twice the tolerance of 0.01) collapses to its midpoint
        // instead: each end moves 0.007.
        let (s, c) = 20f64.to_radians().sin_cos();
        let d = [9.98, -0.006 * c, 0.006 * s].map(|x| x as f32);
        let m = mesh(
            &[
                0.0, 0.0, 0.0, // a
                10.0, 0.0, 0.0, // b
                9.986, 0.0, 0.0, // c
                d[0], d[1], d[2], // d
                10.0, 5.0, 0.0, // e
                0.0, 5.0, 0.0, // f
            ],
            &[0, 1, 2, 1, 4, 2, 4, 5, 2, 5, 0, 2, 1, 0, 3],
        );
        let tolerance = 0.01;
        assert_eq!(degenerate_triangles(&m, tolerance), vec![0]);
        let w = pass(&m, tolerance);
        assert_sound(&m, &w, tolerance);
        assert_eq!(w.left(), 0);
        assert_eq!(w.mesh.triangle_count(), 3);
        assert_eq!(w.vertex_of[1], w.vertex_of[2]);
        let mid = w.mesh.vertex(w.vertex_of[1]).map(f64::from);
        assert!(length(sub(mid, [9.993, 0.0, 0.0])) < 1e-5, "{mid:?}");
        assert!(degenerate_triangles(&w.mesh, tolerance).is_empty());
        assert!(!pass(&w.mesh, tolerance).changed);
    }

    /// The cube's top face as (4, 5, q), (5, 6, q), (6, 7, p), (7, 4, p) with p and q `gap` apart
    /// near its middle, joined by the needles (4, q, p) and (6, p, q).
    fn top_needles(gap: f32) -> Mesh {
        let mut m = cube();
        m.indices.drain(6..12);
        m.positions
            .extend_from_slice(&[10.0, 10.0, 20.0, 10.0 + gap, 10.0, 20.0]);
        let (p, q) = (8, 9);
        m.indices.extend_from_slice(&[
            4, 5, q, 5, 6, q, 6, 7, p, 7, 4, p, //
            4, q, p, 6, p, q,
        ]);
        m
    }

    #[test]
    fn a_needle_collapses_to_its_midpoint() {
        let m = top_needles(0.0001);
        let tolerance = default_tolerance(&m);
        assert_eq!(degenerate_triangles(&m, tolerance), vec![14, 15]);
        let out = remove(&m);
        assert_eq!(out.message, "Removed 2 degenerate faces.");
        assert_eq!(out.mesh.triangle_count(), 14);
        assert_eq!(out.mesh.vertex_count(), 9);
        let mid = (10.0f64 + 0.00005) as f32;
        assert_eq!(out.mesh.vertex(8), [mid, 10.0, 20.0]);
        let s = silent(|c| analyze(&out.mesh, c)).unwrap().status;
        assert!(
            s.closed && s.oriented && s.degenerate_triangles == 0,
            "{s:?}"
        );
        assert_eq!(s.volume, 8000.0);
        assert_eq!(out.origin, Origin::Map((0..14).collect()));
        let w = pass(&m, tolerance);
        assert_eq!(w.vertex_of[8], w.vertex_of[9]);
        assert_sound(&m, &w, tolerance);
        assert!(!remove(&out.mesh).changed);
    }

    #[test]
    fn a_needle_against_a_sharp_edge_collapses_into_the_edge() {
        // The top front edge 4-5 gets a vertex u = (10, 0, 20); v lies 0.0001 below it on the
        // front face, making the needles (5, u, v) and (u, 4, v). u is on the 90° crease and v is
        // not, so v moves to u and the crease stays straight.
        let mut m = cube();
        m.positions
            .extend_from_slice(&[10.0, 0.0, 20.0, 10.0, 0.0, 19.9999]);
        let (u, v) = (8, 9);
        // Drop the cube's +z (6..12) and -y (12..18) faces, then add the new ones.
        m.indices = m.indices[..6]
            .iter()
            .chain(&m.indices[18..])
            .copied()
            .collect();
        m.indices.extend_from_slice(&[
            0, 1, v, 1, 5, v, 5, u, v, u, 4, v, 4, 0, v, // front, y = 0
            4, u, 7, u, 5, 6, u, 6, 7, // top, z = 20
        ]);
        let before = silent(|c| analyze(&m, c)).unwrap().status;
        assert!(before.closed && before.oriented, "{before:?}");
        let tolerance = default_tolerance(&m);
        assert_eq!(degenerate_triangles(&m, tolerance).len(), 2);
        let w = pass(&m, tolerance);
        assert_sound(&m, &w, tolerance);
        let out = &w.mesh;
        assert_eq!(out.vertex(w.vertex_of[u as usize]), [10.0, 0.0, 20.0]);
        assert_eq!(w.vertex_of[u as usize], w.vertex_of[v as usize]);
        let s = silent(|c| analyze(out, c)).unwrap().status;
        assert!(
            s.closed && s.oriented && s.degenerate_triangles == 0,
            "{s:?}"
        );
        assert_eq!(s.volume, 8000.0);
    }

    #[test]
    fn a_zero_area_triangle_splits_the_triangle_across() {
        // m lies exactly on the edge 4-5, so (4, 5, m) has no area; its long edge is shared with
        // the front triangle (0, 5, 4), which gets m as a corner instead.
        let mut m = cube();
        m.positions.extend_from_slice(&[10.0, 0.0, 20.0]);
        m.indices.drain(6..12);
        m.indices
            .extend_from_slice(&[4, 5, 8, 8, 5, 6, 8, 6, 7, 4, 8, 7]);
        let tolerance = default_tolerance(&m);
        assert_eq!(degenerate_triangles(&m, tolerance), vec![10]);
        let w = pass(&m, tolerance);
        assert_sound(&m, &w, tolerance);
        assert_eq!(w.mesh.positions, m.positions, "nothing moves");
        assert_eq!(w.mesh.triangle_count(), 14);
        let s = silent(|c| analyze(&w.mesh, c)).unwrap().status;
        assert!(
            s.closed && s.oriented && s.degenerate_triangles == 0,
            "{s:?}"
        );
        // Even with no tolerance at all, the exactly flat triangle goes.
        let w = pass(&m, 0.0);
        assert_eq!(w.mesh.triangle_count(), 14);
        assert_eq!(w.left(), 0);
    }

    #[test]
    fn turned_over_needles_are_unfolded() {
        // The top face as a fan round a point 0.0005 from corner 5 and just outside the face, so
        // (4, 5, p) faces down and overlaps its neighbours, as rounding can leave a sliver after
        // Wrap part surface; (4, 5, p) and (5, 6, p) are needles over the edge 5-p.
        let mut m = cube();
        m.indices.drain(6..12);
        m.positions.extend_from_slice(&[19.9995, -0.0002, 20.0]);
        for [a, b] in [[4, 5], [5, 6], [6, 7], [7, 4]] {
            m.indices.extend_from_slice(&[a, b, 8]);
        }
        assert!(self_intersections(&m) > 0);
        assert_eq!(
            degenerate_triangles(&m, default_tolerance(&m)),
            vec![10, 11]
        );
        let out = remove(&m);
        assert_eq!(out.message, "Removed 2 degenerate faces.");
        assert_eq!(self_intersections(&out.mesh), 0);
        let s = silent(|c| analyze(&out.mesh, c)).unwrap().status;
        assert!(
            s.closed && s.oriented && s.degenerate_triangles == 0,
            "{s:?}"
        );
        assert_eq!(s.volume, 8000.0);

        // The same in the middle of a flat face: (6, 4, p) with p on the wrong side of the
        // diagonal, 0.0006 from corner 6.
        let mut m = cube();
        m.positions.extend_from_slice(&[19.9995, 19.9997, 20.0]);
        m.indices[6..12].copy_from_slice(&[4, 5, 8, 5, 6, 8]);
        m.indices.extend_from_slice(&[6, 4, 8, 4, 6, 7]);
        assert!(self_intersections(&m) > 0);
        let tolerance = default_tolerance(&m);
        let w = pass(&m, tolerance);
        assert_eq!(self_intersections(&w.mesh), 0);
        let s = silent(|c| analyze(&w.mesh, c)).unwrap().status;
        assert!(
            s.closed && s.oriented && s.degenerate_triangles == 0,
            "{s:?}"
        );
        assert_eq!(s.volume, 8000.0);
    }

    #[test]
    fn thin_triangles_with_their_corners_apart_stay() {
        // As in the reference tool's results: only a short edge, or no area at all, makes a
        // triangle degenerate.
        let m = cube_sliver(0.0002);
        assert!(degenerate_triangles(&m, default_tolerance(&m)).is_empty());
        assert!(!remove(&m).changed);
    }

    #[test]
    fn repeated_corners_are_dropped() {
        let mut m = cube();
        m.indices.extend_from_slice(&[0, 0, 1, 3, 3, 3]);
        let out = remove(&m);
        assert_eq!(out.message, "Removed 2 degenerate faces.");
        assert_eq!(out.mesh, cube());
        assert_eq!(out.origin, Origin::Map((0..12).collect()));
    }

    #[test]
    fn a_needle_that_would_double_a_triangle_stays() {
        // A tetrahedron with two corners 0.0001 apart: collapsing them would leave two triangles
        // on the same three corners.
        let m = mesh(
            &[
                0.0, 0.0, 0.0, 0.0001, 0.0, 0.0, 5.0, 8.0, 0.0, 5.0, 3.0, 8.0,
            ],
            &[0, 2, 1, 0, 1, 3, 1, 2, 3, 0, 3, 2],
        );
        let tolerance = default_tolerance(&m);
        let before = degenerate_triangles(&m, tolerance);
        assert_eq!(before, vec![0, 1]);
        let out = remove(&m);
        assert!(!out.changed);
        assert_eq!(out.mesh, m);
        assert_eq!(out.message, "No degenerate faces could be removed.");
        assert_eq!(
            out.warnings,
            vec!["2 degenerate faces were left because fixing them would tear or fold the mesh."]
        );
    }

    #[test]
    fn border_slivers_keep_the_border() {
        // A flat 10 × 10 sheet of five triangles round a point on its bottom edge, so (0, 1, 4) is
        // a cap on the border, and a needle at its top-right corner.
        let m = mesh(
            &[
                0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 10.0, 10.0, 0.0, 0.0, 10.0, 0.0, // corners
                5.0, 0.0, 0.0, // on the bottom edge
                9.99995, 10.0, 0.0, // just left of corner 2, on the top edge
            ],
            &[0, 1, 4, 1, 2, 4, 2, 5, 4, 5, 3, 4, 3, 0, 4],
        );
        let tolerance = 0.0005;
        let (_, border, _, euler) = topology(&m);
        let w = pass(&m, tolerance);
        assert_sound(&m, &w, tolerance);
        assert_eq!(w.left(), 0);
        assert_eq!(w.mesh.triangle_count(), 3);
        // The border loses the needle's short edge and the cap's long edge becomes two.
        assert_eq!(topology(&w.mesh), (7, border, 0, euler));
        assert!(degenerate_triangles(&w.mesh, tolerance).is_empty());
        // The sheet's corner, where the border turns, stays exactly where it was.
        assert_eq!(w.mesh.vertex(w.vertex_of[2]), [10.0, 10.0, 0.0]);
        assert!((stats(&w.mesh).area - 100.0).abs() < 10.0 * tolerance);
        assert!(!pass(&w.mesh, tolerance).changed);
    }

    #[test]
    fn with_a_selection_only_selected_slivers_change() {
        // Two separate cubes, each with a cap like the cube-sliver fixture.
        let a = cube_sliver(0.0);
        let mut b = cube_sliver(0.0);
        for p in b.positions.as_chunks_mut::<3>().0 {
            p[0] += 30.0;
        }
        let m = crate::ops::concat(&[&a, &b]).unwrap();
        let tolerance = default_tolerance(&m);
        assert_eq!(degenerate_triangles(&m, tolerance), vec![10, 24]);
        let out = silent(|c| run(&m, &[24, 3], &DEGENERATE, c)).unwrap();
        assert_eq!(out.message, "Removed 1 degenerate face.");
        assert!(
            out.warnings.is_empty(),
            "unselected slivers are not reported"
        );
        let left = degenerate_triangles(&out.mesh, default_tolerance(&out.mesh));
        assert_eq!(left, vec![10]);
        // The selection follows: triangle 3 stays triangle 3, the cap 24 is gone.
        assert_eq!(
            remap_selection(&[3, 24], &out.origin, out.mesh.triangle_count()),
            vec![3]
        );
    }

    #[test]
    fn non_manifold_spots_are_left_alone() {
        // A fin glued to the cube-sliver's front top edge makes that edge non-manifold: the cap
        // on it cannot split the triangle across, and nothing there may change.
        let mut m = cube_sliver(0.0);
        m.positions.extend_from_slice(&[10.0, -10.0, 30.0]);
        m.indices.extend_from_slice(&[5, 4, 9]);
        let tolerance = default_tolerance(&m);
        assert_eq!(degenerate_triangles(&m, tolerance), vec![10]);
        let out = remove(&m);
        assert!(!out.changed);
        assert_eq!(
            out.warnings,
            vec![
                "1 degenerate face was left where the mesh is non-manifold or inconsistently oriented."
            ]
        );
        // A turned triangle next to a needle's corner makes its fan inconsistent: the same.
        let mut m = top_needles(0.0001);
        m.indices.swap(12 * 3 + 1, 12 * 3 + 2);
        let out = remove(&m);
        assert!(!out.changed, "{}", out.message);
        assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
    }

    #[test]
    fn the_absolute_tolerance_of_wrap_collapses_only_rounding_slivers() {
        // Wrap part surface's cleanup: slivers far below a micron go, the cube-sliver's 0.0002 mm
        // cap stays at 1e-6 × the diagonal.
        let m = cube_sliver(0.0002);
        let tolerance = 1e-6 * bbox_diagonal(&m);
        let c = silent(|c| collapse(&m, tolerance, None, c)).unwrap();
        assert_eq!((c.removed, c.left), (0, 0));
        assert_eq!(c.mesh, m);
        assert_eq!(c.origin, Origin::Same);
        let m = cube_sliver(1e-6);
        let c = silent(|c| collapse(&m, tolerance, None, c)).unwrap();
        assert_eq!(c.left, 0);
        assert_eq!(c.mesh.triangle_count(), 14);
        assert!(degenerate_triangles(&c.mesh, tolerance).is_empty());
        // `only` limits it like a selection.
        let c = silent(|c| collapse(&m, tolerance, Some(&[0]), c)).unwrap();
        assert_eq!(c.mesh, m);
    }

    /// Cuts slivers into the mesh: in a share of the triangles thicker than `depth` (including ones
    /// made earlier, so slivers also meet) a new corner just inside, at a random height below
    /// `depth` from a corner (two needles) or from an edge (a thin triangle, which is not
    /// degenerate), so the surface keeps its shape, closure and orientation.
    fn with_slivers(m: &Mesh, rng: &mut Rng, share: f64, depth: f64) -> Mesh {
        let mut out = m.clone();
        let mut t = 0;
        while t < out.triangle_count() {
            let c = [0, 1, 2].map(|k| out.indices[t * 3 + k]);
            if rng.chance(share) && shape(c.map(|v| vertex(&out, v)), depth).height >= depth {
                let k = rng.below(3) as usize;
                let (a, b, o) = (c[k], c[(k + 1) % 3], c[(k + 2) % 3]);
                let [pa, pb, po] = [a, b, o].map(|v| vertex(&out, v));
                let foot = if rng.chance(0.7) {
                    pa
                } else {
                    lerp(pa, pb, 0.2 + 0.6 * rng.unit())
                };
                // Toward the centroid, at most halfway, so the corner stays inside.
                let toward = sub(scale(add(add(pa, pb), po), 1.0 / 3.0), foot);
                let reach = length(toward);
                let step = (depth * (0.05 + 0.9 * rng.unit())).min(0.5 * reach);
                let p = add(foot, scale(toward, step / reach));
                out.positions.extend(p.map(|x| x as f32));
                let n = out.vertex_count() as u32 - 1;
                out.indices[t * 3..t * 3 + 3].copy_from_slice(&[a, b, n]);
                out.indices.extend_from_slice(&[b, o, n, o, a, n]);
            }
            t += 1;
        }
        out
    }

    #[test]
    fn random_slivers_on_a_torus_are_removed_soundly() {
        let mut idempotent = 0;
        for seed in 0..16u64 {
            let mut rng = Rng(seed);
            let base = torus(40.0, 15.0, 30, 20);
            let tolerance = 0.002 * bbox_diagonal(&base) / 100.0;
            let share = if seed % 2 == 0 { 0.15 } else { 0.04 };
            let m = with_slivers(&base, &mut rng, share, tolerance);
            let found = degenerate_triangles(&m, tolerance).len();
            assert!(found > 30, "seed {seed}: {found}");
            assert_eq!(self_intersections(&m), 0, "seed {seed}");
            let w = pass(&m, tolerance);
            assert_sound(&m, &w, tolerance);
            let left = degenerate_triangles(&w.mesh, tolerance).len();
            assert_eq!(left, w.left(), "seed {seed}");
            assert!(left * 50 <= found, "seed {seed}: {left} of {found} left");
            // A second run changes nothing unless the displacement bound held a fix back, and then
            // runs settle within a few more.
            let again = pass(&w.mesh, tolerance);
            assert_sound(&w.mesh, &again, tolerance);
            if w.bound == 0 {
                assert!(
                    !again.changed,
                    "seed {seed}: the second run changed the mesh"
                );
                idempotent += 1;
            }
            let mut current = again;
            for _ in 0..3 {
                if !current.changed {
                    break;
                }
                let next = pass(&current.mesh, tolerance);
                assert_sound(&current.mesh, &next, tolerance);
                current = next;
            }
            assert!(!current.changed, "seed {seed}: runs do not settle");
        }
        assert!(idempotent >= 12, "{idempotent}");
    }

    #[test]
    fn many_collapses_never_move_a_vertex_beyond_the_tolerance() {
        // A torus whose edges (about 1.2 mm) are all shorter than the tolerance: every triangle is
        // a needle, and the displacement bound is what stops the collapses.
        let m = torus(40.0, 15.0, 200, 80);
        let tolerance = 2.0;
        let w = pass(&m, tolerance);
        assert!(w.changed);
        assert!(w.mesh.triangle_count() < m.triangle_count() / 2);
        assert_sound(&m, &w, tolerance);
        let s = silent(|c| analyze(&w.mesh, c)).unwrap().status;
        assert!(s.closed && s.oriented, "{s:?}");
        let v0 = stats(&m).volume;
        assert!((s.volume - v0).abs() < 0.02 * v0, "{} -> {}", v0, s.volume);
        // Most of what is left is held back by the bound alone.
        assert!(w.stuck <= w.bound, "{} stuck, {} bound", w.stuck, w.bound);
    }

    /// A flat square (two triangles, z = 0) and, above it, a separate triangle whose corner 6 sits
    /// at height `z` over the square's middle.
    fn square_and_triangle(z: f32) -> Mesh {
        mesh(
            &[
                0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 10.0, 10.0, 0.0, 0.0, 10.0, 0.0, // square
                3.0, 3.0, 1.0, 7.0, 3.0, 1.0, 5.0, 6.0, z, // triangle
            ],
            &[0, 1, 2, 0, 2, 3, 4, 5, 6],
        )
    }

    #[test]
    fn a_change_that_crosses_the_surface_is_caught_and_damage_is_not() {
        // The triangle clears the square in the input; then its corner 6 goes down through it as
        // a fix might move it. The check finds the crossing and names the input vertices to keep.
        let m = square_and_triangle(1.0);
        let none = vec![false; m.vertex_count()];
        let mut s = Surface::new(&m, 0.001, None, &[], &none);
        s.pos[6] = [5.0, 6.0, -1.0];
        s.touched[2] = true;
        let (vertices, crossed) = silent(|c| s.crossings(&m, c)).unwrap();
        assert_eq!(crossed, 1);
        assert_eq!(vertices, vec![4, 5, 6]);
        // When the input already had the triangle through the square, a changed triangle there
        // continues that damage and does not count.
        let m = square_and_triangle(-1.0);
        let mut s = Surface::new(&m, 0.001, None, &[], &none);
        s.touched[2] = true;
        let (vertices, crossed) = silent(|c| s.crossings(&m, c)).unwrap();
        assert_eq!((vertices, crossed), (Vec::new(), 0));
        // Nothing changed, nothing to check.
        let s = Surface::new(&m, 0.001, None, &[], &none);
        assert_eq!(silent(|c| s.crossings(&m, c)).unwrap(), (Vec::new(), 0));
        // Neighbours on a shared edge facing the same way are spared the exact test, and do not
        // cross.
        let cube = cube();
        let mut s = Surface::new(&cube, 0.001, None, &[], &[false; 8]);
        s.touched.fill(true);
        assert_eq!(silent(|c| s.crossings(&cube, c)).unwrap(), (Vec::new(), 0));
        // Two 10 × 10 grids lying on each other half a cell apart overlap everywhere: every crossing
        // continues one the input had, so none counts.
        let grid = |shift: f32| {
            let mut positions = Vec::new();
            let mut indices = Vec::new();
            for j in 0..11u32 {
                for i in 0..11u32 {
                    positions.extend_from_slice(&[i as f32 + shift, j as f32 + shift, 0.0]);
                }
            }
            for j in 0..10u32 {
                for i in 0..10u32 {
                    let a = j * 11 + i;
                    indices.extend_from_slice(&[a, a + 1, a + 12, a, a + 12, a + 11]);
                }
            }
            mesh(&positions, &indices)
        };
        let both = crate::ops::concat(&[&grid(0.0), &grid(0.5)]).unwrap();
        let mut s = Surface::new(&both, 0.001, None, &[], &vec![false; both.vertex_count()]);
        s.touched.fill(true);
        assert_eq!(silent(|c| s.crossings(&both, c)).unwrap(), (Vec::new(), 0));
    }

    #[test]
    fn many_new_crossings_are_all_caught() {
        // 200 small triangles clear above a big square, each then pushed down through it as a fix
        // might move it: every one is caught, however many a pass makes.
        let mut positions = vec![
            0.0, 0.0, 0.0, 100.0, 0.0, 0.0, 100.0, 100.0, 0.0, 0.0, 100.0, 0.0,
        ];
        let mut indices = vec![0, 1, 2, 0, 2, 3];
        for k in 0..200u32 {
            let (x, y) = ((k % 20) as f32 * 5.0 + 1.0, (k / 20) as f32 * 5.0 + 1.0);
            let v = positions.len() as u32 / 3;
            positions.extend_from_slice(&[x, y, 1.0, x + 2.0, y, 1.0, x + 1.0, y + 2.0, 1.0]);
            indices.extend_from_slice(&[v, v + 1, v + 2]);
        }
        let m = mesh(&positions, &indices);
        let none = vec![false; m.vertex_count()];
        let mut s = Surface::new(&m, 0.001, None, &[], &none);
        for t in 2..202u32 {
            let v = s.tri[t as usize][2];
            s.pos[v as usize][2] = -1.0;
            s.touched[t as usize] = true;
        }
        let (vertices, crossed) = silent(|c| s.crossings(&m, c)).unwrap();
        assert_eq!((vertices.len(), crossed), (600, 200));
    }

    #[test]
    fn zero_area_triangles_no_fix_can_reach_are_flipped_as_wrap_flips_them() {
        // The zero-area triangle (4, 5, 8) of the cube's top face, with a small closed tetrahedron
        // touching the cube at 8: 8's triangles make two fans, so no fix may touch the triangle.
        // A later pass flips it into the front triangle across its long edge, as Wrap part surface
        // would before it starts, so Wrap finds nothing to flip after this action, and a second
        // run changes nothing.
        let mut m = cube();
        m.positions.extend_from_slice(&[10.0, 0.0, 20.0]);
        m.indices.drain(6..12);
        m.indices
            .extend_from_slice(&[4, 5, 8, 8, 5, 6, 8, 6, 7, 4, 8, 7]);
        m.positions
            .extend_from_slice(&[9.0, 0.5, 21.0, 11.0, 0.5, 21.0, 10.0, 1.5, 21.0]);
        m.indices
            .extend_from_slice(&[9, 10, 11, 8, 10, 9, 8, 11, 10, 8, 9, 11]);
        let tolerance = default_tolerance(&m);
        assert_eq!(degenerate_triangles(&m, tolerance), vec![10]);
        assert!(!pass(&m, tolerance).changed, "no fix reaches the triangle");
        let w = silent(|c| settled(&m, tolerance, c)).unwrap();
        assert_sound(&m, &w, tolerance);
        assert_eq!(w.mesh.positions, m.positions, "nothing moves");
        assert_eq!((w.found, w.left()), (1, 0));
        assert!(degenerate_triangles(&w.mesh, tolerance).is_empty());
        assert!(super::super::wrap::zip_collinear(&w.mesh).is_none());
        let out = remove(&m);
        assert_eq!(out.message, "Removed 1 degenerate face.");
        assert!(!remove(&out.mesh).changed);
    }

    #[test]
    fn frozen_vertices_stay_where_they_are() {
        // Both ends of the top needles' short edge frozen: the collapse that would move them is
        // refused, and both faces are reported as left to keep the surface from crossing itself.
        let m = top_needles(0.0001);
        let mut frozen = vec![false; m.vertex_count()];
        frozen[8] = true;
        frozen[9] = true;
        let found = [(14u32, 0.0), (15, 0.0)];
        let mut s = Surface::new(&m, default_tolerance(&m), None, &found, &frozen);
        silent(|c| s.settle(c)).unwrap();
        let w = s.finish(&m, 2);
        assert!(!w.changed);
        assert_eq!((w.crossing, w.left()), (2, 2));
    }

    #[test]
    fn what_the_bound_held_back_is_reported_and_a_second_run_changes_nothing() {
        // The same all-needle torus through the action with an absolute tolerance. The action
        // repeats passes until one changes nothing, so a second run has nothing left to do.
        let m = torus(40.0, 15.0, 200, 80);
        let params = DegenerateParams {
            calc_tolerance: false,
            tolerance: 2.0,
        };
        let out = silent(|c| run(&m, &[], &params, c)).unwrap();
        assert!(out.changed);
        let w = silent(|c| settled(&m, 2.0, c)).unwrap();
        assert_eq!(out.mesh, w.mesh);
        assert_eq!(w.found, pass(&m, 2.0).found, "found counts the first pass");
        assert_eq!(
            out.message,
            format!("Removed {}.", faces(w.found - w.left()))
        );
        if w.bound > 0 {
            assert_eq!(
                out.warnings[0],
                format!(
                    "{} left because fixing them would move the surface by more than the tolerance \
                     (2.00000 mm).",
                    faces_were(w.bound)
                )
            );
        }
        assert_eq!(
            degenerate_triangles(&out.mesh, 2.0).len(),
            w.left(),
            "every face left is reported"
        );
        let again = silent(|c| run(&out.mesh, &[], &params, c)).unwrap();
        assert!(!again.changed, "the second run changed: {}", again.message);
    }

    #[test]
    fn a_cancelled_run_stops() {
        let cancel = AtomicBool::new(true);
        let mut control = Control::new(&cancel, |_| {});
        assert_eq!(
            run(&cube_sliver(0.0002), &[], &DEGENERATE, &mut control),
            Err(RepairError::Cancelled)
        );
    }

    #[test]
    fn progress_reaches_the_end_and_never_goes_back() {
        let mut rng = Rng(5);
        let base = torus(40.0, 15.0, 60, 40);
        let m = with_slivers(&base, &mut rng, 0.3, 0.001);
        let never = AtomicBool::new(false);
        let mut seen = Vec::new();
        {
            let mut control = Control::new(&never, |f| seen.push(f));
            run(&m, &[], &DEGENERATE, &mut control).unwrap();
        }
        assert!(seen.windows(2).all(|w| w[1] >= w[0]), "{seen:?}");
        assert_eq!(seen.last(), Some(&1.0));
    }

    /// `cargo test --release --lib repair::degenerate -- --ignored --nocapture`: a 730k-triangle
    /// torus with 3% of its triangles cut into slivers, as raw CAD exports have.
    #[test]
    #[ignore]
    fn timing_on_730k_triangles() {
        let mut rng = Rng(9);
        let base = torus(40.0, 15.0, 600, 590);
        let tolerance = 0.002 * bbox_diagonal(&base) / 100.0;
        let m = with_slivers(&base, &mut rng, 0.011, tolerance);
        let found = degenerate_triangles(&m, tolerance).len();
        let started = Instant::now();
        let out = remove(&m);
        let took = started.elapsed().as_secs_f64();
        println!(
            "remove degenerate faces: {} triangles, {found} degenerate, {took:.3} s: {} {:?}",
            m.triangle_count(),
            out.message,
            out.warnings
        );
        let started = Instant::now();
        let again = remove(&out.mesh);
        println!(
            "  second run {:.3} s: {}",
            started.elapsed().as_secs_f64(),
            again.message
        );
        let started = Instant::now();
        let clean = remove(&base);
        println!(
            "  clean torus ({} triangles) {:.3} s: {}",
            base.triangle_count(),
            started.elapsed().as_secs_f64(),
            clean.message
        );
        assert!(!again.changed);
        if !cfg!(debug_assertions) {
            assert!(took < 0.3, "{took:.3} s");
        }
    }

    /// The finely tessellated case: a 2.5M-triangle torus whose edges are a third of the
    /// tolerance, so nearly every triangle is a needle and the displacement bound decides how far
    /// the collapses go.
    #[test]
    #[ignore]
    fn timing_on_a_dense_2_5m_triangle_surface() {
        let base = torus(40.0, 15.0, 1580, 790);
        // Edges are 0.12 to 0.22 mm.
        let tolerance = 0.36;
        let found = degenerate_triangles(&base, tolerance).len();
        let started = Instant::now();
        let w = pass(&base, tolerance);
        let took = started.elapsed().as_secs_f64();
        println!(
            "dense: {} triangles, {found} degenerate ({:.1}%), {took:.3} s: {} triangles left, \
             {} degenerate left ({} by the bound, {} tangled, {} stuck)",
            base.triangle_count(),
            100.0 * found as f64 / base.triangle_count() as f64,
            w.mesh.triangle_count(),
            w.left(),
            w.bound,
            w.tangled,
            w.stuck
        );
        let mut worst = 0.0f64;
        for (v, &to) in w.vertex_of.iter().enumerate() {
            if to != NONE {
                worst = worst.max(length(sub(vertex(&base, v as u32), vertex(&w.mesh, to))));
            }
        }
        let (_, b, n, e0) = topology(&base);
        let (_, b1, n1, e1) = topology(&w.mesh);
        println!(
            "  farthest vertex move {worst:.4} mm, border {b}->{b1}, non-manifold {n}->{n1}, euler {e0}->{e1}"
        );
        assert!(worst <= tolerance * (1.0 + 1e-9));
        assert_eq!((b1, n1, e1), (0, 0, e0));
        let started = Instant::now();
        let again = pass(&w.mesh, tolerance);
        println!(
            "  second run {:.3} s: changed {}, {} triangles, {} degenerate left",
            started.elapsed().as_secs_f64(),
            again.changed,
            again.mesh.triangle_count(),
            again.left()
        );
    }

    /// Real files run through Extended repair up to Wrap part surface, then this action as its next
    /// step: self-intersecting pairs before and after it (it must add none), time, report, and
    /// whether a second run changes anything. Needs every action of the script, so it runs on the
    /// merged tree: `TRICKY3D_MODELS="a.stl;b.stl" cargo test --release --lib
    /// repair::degenerate::tests::after_wrap -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn after_wrap() {
        use crate::repair::defaults::{EXTENDED_ID, builtin_script};
        use crate::repair::{run_action, scripts::parse_action};
        let Ok(list) = std::env::var("TRICKY3D_MODELS") else {
            return;
        };
        let script = builtin_script(EXTENDED_ID).unwrap();
        let up_to_wrap: Vec<_> = script.actions[..9]
            .iter()
            .map(|s| parse_action(s).unwrap())
            .collect();
        for path in list.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let imported = crate::formats::import_file(
                std::path::Path::new(path),
                true,
                &mut crate::progress::Progress::silent(),
            )
            .unwrap();
            for part in &imported.parts {
                let mut m = part.mesh.clone();
                for action in &up_to_wrap {
                    let out = silent(|c| run_action(action, &m, &[], c)).unwrap();
                    if out.changed {
                        m = out.mesh;
                    }
                }
                let before = self_intersections(&m);
                let started = Instant::now();
                let out = remove(&m);
                let took = started.elapsed().as_secs_f64();
                let after = self_intersections(&out.mesh);
                let again = remove(&out.mesh);
                println!(
                    "{path}: after Wrap {} triangles; {took:.3} s: {} {:?}; self-intersecting \
                     pairs {before} -> {after}; a second run changes {}",
                    m.triangle_count(),
                    out.message,
                    out.warnings,
                    if again.changed { "more" } else { "nothing" }
                );
                assert!(after <= before);
            }
        }
    }

    /// Real files, with timings: `TRICKY3D_MODELS="a.stl;b.stl" cargo test --release --lib
    /// repair::degenerate::tests::real_models -- --ignored --nocapture`. Set TRICKY3D_SOUND=1 to
    /// also compare self-intersections before and after (slow on big files), and
    /// TRICKY3D_REPEAT=n to time the fastest of n runs.
    #[test]
    #[ignore]
    fn real_models() {
        let Ok(list) = std::env::var("TRICKY3D_MODELS") else {
            return;
        };
        let sound = std::env::var("TRICKY3D_SOUND").is_ok();
        let repeat: usize = std::env::var("TRICKY3D_REPEAT")
            .ok()
            .and_then(|r| r.parse().ok())
            .unwrap_or(1);
        for path in list.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let imported = crate::formats::import_file(
                std::path::Path::new(path),
                true,
                &mut crate::progress::Progress::silent(),
            )
            .unwrap();
            for part in &imported.parts {
                let m = &part.mesh;
                let tolerance = default_tolerance(m);
                let found = degenerate_triangles(m, tolerance).len();
                // The fastest of TRICKY3D_REPEAT runs, since other work on the machine slows some.
                let mut took = f64::INFINITY;
                let mut w = pass(m, tolerance);
                for _ in 0..repeat {
                    let started = Instant::now();
                    w = pass(m, tolerance);
                    took = took.min(started.elapsed().as_secs_f64());
                }
                println!(
                    "{path}: {} triangles, tolerance {tolerance:.5} mm, {found} degenerate \
                     ({:.1}%); {took:.3} s: {} fixed, {} left ({} by the bound, {} tangled, {} \
                     stuck), {} triangles",
                    m.triangle_count(),
                    100.0 * found as f64 / m.triangle_count() as f64,
                    found.saturating_sub(w.left()),
                    w.left(),
                    w.bound,
                    w.tangled,
                    w.stuck,
                    w.mesh.triangle_count()
                );
                let started = Instant::now();
                let again = pass(&w.mesh, tolerance);
                println!(
                    "  second run {:.3} s: changed {}, {} left, {} triangles",
                    started.elapsed().as_secs_f64(),
                    again.changed,
                    again.left(),
                    again.mesh.triangle_count()
                );
                let mut worst = 0.0f64;
                for (v, &to) in w.vertex_of.iter().enumerate() {
                    if to != NONE {
                        worst = worst.max(length(sub(vertex(m, v as u32), vertex(&w.mesh, to))));
                    }
                }
                println!("  farthest vertex move {worst:.6} mm");
                assert!(worst <= tolerance * (1.0 + 1e-9));
                let (_, b0, n0, e0) = topology(m);
                let (_, b1, n1, e1) = topology(&w.mesh);
                println!("  border {b0} -> {b1}, non-manifold {n0} -> {n1}, euler {e0} -> {e1}");
                assert!(b1 <= b0 && n1 <= n0);
                let s0 = silent(|c| analyze(m, c)).unwrap().status;
                let s1 = silent(|c| analyze(&w.mesh, c)).unwrap().status;
                println!(
                    "  inverted {} -> {}, misoriented edges {} -> {}, holes {} -> {}, volume \
                     {:.3} -> {:.3} mm³",
                    s0.inverted_triangles,
                    s1.inverted_triangles,
                    misoriented(m),
                    misoriented(&w.mesh),
                    s0.holes,
                    s1.holes,
                    s0.volume,
                    s1.volume
                );
                assert!(misoriented(&w.mesh) <= misoriented(m));
                if sound {
                    let (s0, s1) = (self_intersections(m), self_intersections(&w.mesh));
                    println!("  self-intersecting pairs {s0} -> {s1}");
                }
            }
        }
    }
}
