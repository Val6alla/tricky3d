//! Generate shell (Home › Generate shell, Modify › Generate shell): a new part from a closed part's
//! offset surface. The part's distance field is sampled on a grid of the Accuracy's step, in a
//! narrow band round the surface (src/ops/shell/field.rs); the offset surface is the boundary of the
//! grid points within the thickness, built by marching tetrahedra (src/repair/levelset.rs), so it
//! is closed, oriented and free of crossings by construction; then it is tidied and, if asked,
//! smoothed (src/ops/shell/tidy.rs), and the result goes through the repair checks.
//!
//! Hollow part, improved builds the same result another way, for thin walls and sharp features:
//! - it drops the parts of the cavity thinner than about two grid steps (an opening of the grid
//!   solid), where the plain method leaves slivers and perforated sheets;
//! - it puts every vertex on the true offset surface by searching along its grid edge, where the
//!   plain method interpolates two distances in a straight line (wrong across the ridges of the
//!   distance field, inside thin walls and round edges);
//! - it rebuilds the cavity's sharp edges and corners, which marching tetrahedra cuts off, by
//!   collapsing edges onto the offset surface's own tangent planes (src/ops/shell/tidy.rs).

pub(crate) mod field;
mod tidy;

use serde::{Deserialize, Serialize};

use self::field::{Offset, Surface};
use crate::mesh::Mesh;
use crate::repair::bvh::{self, Aabb};
use crate::repair::exact::{self, P3, sub};
use crate::repair::levelset::{self, CLEAR, CLEAR_MIN, Grid, MIN_CELL};
use crate::repair::{Control, RepairError, RepairResult, status, tritri};

/// `ShellKind` in src/shell/types.ts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShellKind {
    /// The part with an inward hollowing shell, as one part.
    HollowPart,
    /// A leaner part: the inner offset surface.
    InnerOffset,
    /// A thicker part: the outer offset surface, convex edges rounded.
    OuterOffset,
    /// The inner offset surface turned inside out, alone.
    HollowingShell,
    /// Hollow part by the second method (module comment).
    HollowPartImproved,
}

impl ShellKind {
    fn offset(self, thickness: f64) -> Offset {
        match self {
            ShellKind::OuterOffset => Offset::Outer(thickness),
            _ => Offset::Inner(thickness),
        }
    }

    fn hollow(self) -> bool {
        matches!(self, ShellKind::HollowPart | ShellKind::HollowPartImproved)
    }

    fn improved(self) -> bool {
        self == ShellKind::HollowPartImproved
    }
}

/// `ShellSettings` in src/shell/types.ts: the dialog's fields, mm.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShellSettings {
    pub kind: ShellKind,
    pub thickness: f64,
    /// The grid step.
    pub accuracy: f64,
    /// Round the new surface's corners for an even mesh.
    pub smooth: bool,
}

/// Thickest shell, mm.
pub const MAX_THICKNESS: f64 = 1000.0;
/// Most grid points sampled: the inside map and the solid each hold a bit per point.
pub const MAX_POINTS: f64 = 4.0e9;

/// The sentence for settings the dialog should not have let through; None when they are fine.
pub fn settings_problem(s: &ShellSettings) -> Option<String> {
    let (t, h) = (s.thickness, s.accuracy);
    if !(t.is_finite() && t > 0.0 && t <= MAX_THICKNESS) {
        return Some(format!(
            "Enter a shell thickness greater than 0 and up to {MAX_THICKNESS} mm."
        ));
    }
    if !(h.is_finite() && h >= MIN_CELL) {
        return Some(format!("Enter an accuracy of at least {MIN_CELL} mm."));
    }
    if h > t / 2.0 {
        return Some(format!(
            "Enter an accuracy of at most half the thickness ({} mm).",
            round_mm(t / 2.0)
        ));
    }
    None
}

/// One part's shell: the new part's mesh (world space), or None when nothing is left of it, and
/// what the user should know.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shell {
    pub mesh: Option<Mesh>,
    pub warnings: Vec<String>,
}

/// Generates the shell of `mesh`, a closed and oriented part in world space (the caller checks).
/// Fails with a sentence when the result does not pass the repair checks.
pub fn generate(mesh: &Mesh, s: &ShellSettings, control: &mut Control) -> RepairResult<Shell> {
    if let Some(problem) = settings_problem(s) {
        return Err(RepairError::Failed(problem));
    }
    let (t, h) = (s.thickness, s.accuracy);
    let offset = s.kind.offset(t);
    let surface = Surface::new(mesh);
    let Some(bounds) = mesh_bounds(mesh) else {
        return Ok(Shell::default());
    };
    let grow = match offset {
        Offset::Outer(_) => t + 2.0 * h,
        Offset::Inner(_) => 0.0,
    };
    let bounds = [bounds[0].map(|x| x - grow), bounds[1].map(|x| x + grow)];
    if grid_points(&bounds, h) > MAX_POINTS {
        return Err(RepairError::Failed(too_fine(h)));
    }
    let made = offset_surface(
        &surface,
        &bounds,
        offset,
        h,
        s.kind.improved(),
        &mut control.sub(0.0, 0.75),
    )?;
    let mut warnings = Vec::new();
    let Some(made) = made else {
        warnings.push(nothing_left(s.kind, t));
        return Ok(Shell {
            mesh: None,
            warnings,
        });
    };
    control.stage("Tidying the mesh");
    let settings = tidy::Settings {
        cell: h,
        offset,
        features: s.kind.improved() && !s.smooth,
        smooth: s.smooth,
        longest: if s.kind.hollow() { t } else { f64::INFINITY },
    };
    let tidied = tidy::tidy(made, &surface, &settings, &mut control.sub(0.75, 0.85))?;
    if tidied.untouched && s.smooth {
        warnings.push(
            "The result could not be smoothed without its triangles crossing, so it is left as \
             built."
                .into(),
        );
    }
    let made = tidied.mesh;
    if matches!(offset, Offset::Inner(_))
        && let Some(thin) = thin_walls(&surface, &made, t, h, control.cancel_flag())
    {
        warnings.push(thin_message(s.kind, t, &thin, &bounds_of(&surface)));
    }
    let first_new = if s.kind.hollow() {
        mesh.triangle_count()
    } else {
        0
    };
    // A hollowing shell is checked before it is turned: inside out, it faces the wrong way on
    // purpose.
    let result = match s.kind {
        ShellKind::OuterOffset | ShellKind::InnerOffset | ShellKind::HollowingShell => made,
        ShellKind::HollowPart | ShellKind::HollowPartImproved => {
            super::concat(&[mesh, &turned(made)]).map_err(|reason| {
                RepairError::Failed(format!("Could not generate the shell: {reason}."))
            })?
        }
    };
    control.stage("Checking the result");
    let checked = verify(&result, first_new, &mut control.sub(0.85, 1.0))?;
    let result = match s.kind {
        ShellKind::HollowingShell => turned(result),
        _ => result,
    };
    if let Some(problem) = checked.problem() {
        return Err(RepairError::Failed(problem));
    }
    if checked.old_crossings {
        warnings.push(
            "The original part has triangles that cross each other, and the result keeps them. \
             Repair the part to remove them."
                .into(),
        );
    }
    control.progress(1.0);
    Ok(Shell {
        mesh: Some(result),
        warnings,
    })
}

/// The offset surface, facing out of the offset solid; None when the solid is empty. Also Modify's
/// Manifold remeshing (src/modify/remesh.rs).
pub(crate) fn offset_surface(
    surface: &Surface,
    bounds: &Aabb,
    offset: Offset,
    cell: f64,
    improved: bool,
    control: &mut Control,
) -> RepairResult<Option<Mesh>> {
    let grid = Grid::new(bounds, cell);
    control.stage("Finding the inside");
    let inside = grid.classify(
        surface.points(),
        surface.triangles(),
        None,
        control.cancel_flag(),
    );
    control.check()?;
    control.progress(0.2);
    control.stage("Measuring distances");
    let mut solid = field::solid(&grid, &inside, surface, offset, &mut control.sub(0.2, 0.75))?;
    drop(inside);
    if improved {
        solid = field::open(&grid, &solid, control.cancel_flag());
        control.check()?;
    }
    control.stage("Building the surface");
    // Every grid edge the surface crosses lies within a diagonal step of it.
    let cap = 2.0 * cell;
    let tolerance = 1e-4 * cell;
    let place = |key: u64| -> P3 {
        let [a, b] = grid.ends(key);
        let (inner, outer) = if grid.at(&solid, a) { (a, b) } else { (b, a) };
        let (p, q) = (grid.point(inner), grid.point(outer));
        let d = sub(q, p);
        let at = |s: f64| [0, 1, 2].map(|k| p[k] + s * d[k]);
        let level = |x: P3| field::level(surface, offset, x, cap);
        let (g0, g1) = (level(p), level(q));
        let s = if g0 <= 0.0 && g1 > 0.0 {
            if improved {
                root(|s| level(at(s)), g0, g1, tolerance)
            } else {
                -g0 / (g1 - g0)
            }
        } else {
            // An edge the opening cut through, between two points of the solid.
            0.5
        };
        let clear = (CLEAR / exact::length(d)).clamp(CLEAR_MIN, 0.5);
        at(s.clamp(clear, 1.0 - clear))
    };
    levelset::surface(&grid, &solid, &mut control.sub(0.75, 1.0), place)
}

/// Where `f` changes sign on 0..1, given f(0) = `f0` ≤ 0 < f(1) = `f1`: regula falsi with the
/// Illinois step, to `tolerance` in f.
fn root(f: impl Fn(f64) -> f64, f0: f64, f1: f64, tolerance: f64) -> f64 {
    let (mut a, mut b, mut fa, mut fb) = (0.0, 1.0, f0, f1);
    let mut kept = 0;
    for _ in 0..16 {
        let s = (a * fb - b * fa) / (fb - fa);
        let fs = f(s);
        if fs.abs() <= tolerance {
            return s;
        }
        if fs > 0.0 {
            (b, fb) = (s, fs);
            if kept == -1 {
                fa /= 2.0;
            }
            kept = -1;
        } else {
            (a, fa) = (s, fs);
            if kept == 1 {
                fb /= 2.0;
            }
            kept = 1;
        }
        if b - a < 1e-9 {
            break;
        }
    }
    (a * fb - b * fa) / (fb - fa)
}

/// The mesh with every triangle turned the other way.
fn turned(mut mesh: Mesh) -> Mesh {
    for t in mesh.indices.as_chunks_mut::<3>().0 {
        t.swap(1, 2);
    }
    mesh
}

fn mesh_bounds(mesh: &Mesh) -> Option<Aabb> {
    let mut b = bvh::EMPTY;
    for v in 0..mesh.vertex_count() as u32 {
        bvh::include(&mut b, exact::vertex(mesh, v));
    }
    (mesh.triangle_count() > 0 && b[0][0] <= b[1][0]).then_some(b)
}

fn bounds_of(surface: &Surface) -> Aabb {
    let mut b = bvh::EMPTY;
    for &p in surface.points() {
        bvh::include(&mut b, p);
    }
    b
}

/// Grid points over `bounds` at step `cell`, with levelset's padding.
fn grid_points(bounds: &Aabb, cell: f64) -> f64 {
    (0..3)
        .map(|a| ((bounds[1][a] - bounds[0][a]) / cell + 5.0).ceil())
        .product()
}

fn too_fine(cell: f64) -> String {
    format!(
        "An accuracy of {} mm is too fine for a part this size. Enter a larger value.",
        round_mm(cell)
    )
}

/// Millimetres for a message: at most three decimals, no trailing zeros.
fn round_mm(x: f64) -> String {
    let s = format!("{x:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn nothing_left(kind: ShellKind, t: f64) -> String {
    let twice = round_mm(2.0 * t);
    match kind {
        ShellKind::HollowPart | ShellKind::HollowPartImproved => format!(
            "No cavity fits: every wall of the part is thinner than {twice} mm, twice the shell \
             thickness. No part was made."
        ),
        _ => format!(
            "Nothing is left of the part: every wall is thinner than {twice} mm, twice the shell \
             thickness. No part was made."
        ),
    }
}

// ---------------------------------------------------------------------------------------------
// Thin walls

/// Where an inner offset left out part of the model.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ThinWalls {
    /// Share of the part's surface area, 0..1.
    pub share: f64,
    /// Box round the triangles concerned.
    pub bounds: Aabb,
    /// Their area-weighted centre.
    pub centre: P3,
}

/// The part's surface that is far from the inner offset surface: walls thinner than twice the
/// thickness, which the inner offset leaves out. A point on the surface lies the thickness from the
/// inner offset where the wall is thick enough, up to √3 times it at a convex corner; one more
/// than 1.8 times the thickness (and two grid steps) away has nothing of the offset behind it.
/// None when that is under 0.2 % of the surface.
pub(crate) fn thin_walls(
    original: &Surface,
    inner: &Mesh,
    t: f64,
    cell: f64,
    cancel: &std::sync::atomic::AtomicBool,
) -> Option<ThinWalls> {
    use rayon::prelude::*;
    let offset = Surface::new(inner);
    let reach = 1.8 * t + 2.0 * cell;
    let points = original.points();
    let (area, flagged) = original
        .triangles()
        .par_iter()
        .map(|tri| {
            let [a, b, c] = tri.map(|v| points[v as usize]);
            let area = exact::length(exact::cross(sub(b, a), sub(c, a))) / 2.0;
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return (area, None);
            }
            let centre = [0, 1, 2].map(|k| (a[k] + b[k] + c[k]) / 3.0);
            let far = offset.is_empty() || offset.nearest(centre, reach * reach).is_none();
            (area, far.then_some((area, [a, b, c], centre)))
        })
        .fold(
            || (0.0, None::<(f64, Aabb, P3)>),
            |(total, acc), (area, hit)| {
                let acc = match hit {
                    None => acc,
                    Some((w, corners, centre)) => {
                        let (mut sum, mut b, mut c) = acc.unwrap_or((0.0, bvh::EMPTY, [0.0; 3]));
                        corners.iter().for_each(|&p| bvh::include(&mut b, p));
                        for k in 0..3 {
                            c[k] += w * centre[k];
                        }
                        sum += w;
                        Some((sum, b, c))
                    }
                };
                (total + area, acc)
            },
        )
        .reduce(
            || (0.0, None),
            |(ta, a), (tb, b)| {
                let merged = match (a, b) {
                    (None, x) | (x, None) => x,
                    (Some((wa, mut ba, ca)), Some((wb, bb, cb))) => {
                        bvh::grow(&mut ba, &bb);
                        Some((wa + wb, ba, [0, 1, 2].map(|k| ca[k] + cb[k])))
                    }
                };
                (ta + tb, merged)
            },
        );
    let (sum, bounds, centre) = flagged?;
    let share = if area > 0.0 { sum / area } else { 1.0 };
    (share >= 0.002 && sum > 0.0).then(|| ThinWalls {
        share,
        bounds,
        centre: centre.map(|x| x / sum),
    })
}

/// "Walls thinner than 4 mm stay solid: about 12 % of the surface, at the top of the part (X 0 to
/// 20, Y 0 to 3, Z 15 to 20 mm)."
fn thin_message(kind: ShellKind, t: f64, thin: &ThinWalls, part: &Aabb) -> String {
    let what = match kind {
        ShellKind::HollowPart | ShellKind::HollowPartImproved => "stay solid, with no cavity",
        ShellKind::HollowingShell => "get no hollowing shell",
        _ => "are left out of the inner offset",
    };
    let percent = (thin.share * 100.0).round().max(1.0);
    let b = &thin.bounds;
    let twice = round_mm(2.0 * t);
    // Spread over most of the part's box in every direction, a box says nothing.
    let everywhere = (0..3).all(|k| b[1][k] - b[0][k] >= 0.8 * (part[1][k] - part[0][k]));
    if everywhere {
        return format!(
            "Walls thinner than {twice} mm (twice the shell thickness) {what}: about {percent} % \
             of the surface, all over the part."
        );
    }
    // A tenth of a millimetre is plenty to say where.
    let at = |x: f64| round_mm((x * 10.0).round() / 10.0);
    format!(
        "Walls thinner than {twice} mm (twice the shell thickness) {what}: about {percent} % of \
         the surface, {} (X {} to {}, Y {} to {}, Z {} to {} mm).",
        side(thin.centre, part),
        at(b[0][0]),
        at(b[1][0]),
        at(b[0][1]),
        at(b[1][1]),
        at(b[0][2]),
        at(b[1][2]),
    )
}

/// Which side of the part `p` lies toward: "at the top of the part", "toward −X", "in the middle".
pub(crate) fn side(p: P3, part: &Aabb) -> &'static str {
    let rel = [0, 1, 2].map(|k| {
        let half = (part[1][k] - part[0][k]) / 2.0;
        if half > 0.0 {
            (p[k] - (part[0][k] + half)) / half
        } else {
            0.0
        }
    });
    let (axis, value) = rel
        .iter()
        .copied()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap_or((0, 0.0));
    if value.abs() < 0.3 {
        return "in the middle of the part";
    }
    match (axis, value > 0.0) {
        (0, true) => "toward the +X side of the part",
        (0, false) => "toward the −X side of the part",
        (1, true) => "toward the +Y side of the part",
        (1, false) => "toward the −Y side of the part",
        (_, true) => "at the top of the part",
        (_, false) => "at the bottom of the part",
    }
}

// ---------------------------------------------------------------------------------------------
// Checks

/// What the repair checks found in a result.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Verified {
    pub closed: bool,
    pub oriented: bool,
    /// Crossing pairs with a generated triangle in them.
    pub new_crossings: usize,
    /// Crossing pairs of the original part's own triangles (Hollow part keeps them).
    pub old_crossings: bool,
}

impl Verified {
    fn problem(&self) -> Option<String> {
        let mut what = Vec::new();
        if !self.closed {
            what.push("is not closed");
        }
        if !self.oriented {
            what.push("is not oriented");
        }
        if self.new_crossings > 0 {
            what.push("has crossing triangles");
        }
        (!what.is_empty()).then(|| {
            format!(
                "Generate shell made a result that {}, so no part was added. Try a smaller \
                 accuracy value.",
                what.join(" and ")
            )
        })
    }
}

/// Closed, oriented and free of crossings, as Repair's Status tab and Detect self-intersections
/// tell; triangles from `first_new` on are the generated ones.
pub(crate) fn verify(
    mesh: &Mesh,
    first_new: usize,
    control: &mut Control,
) -> RepairResult<Verified> {
    let status = status::analyze(mesh, &mut control.sub(0.0, 0.3))?.status;
    let pairs = tritri::intersecting_pairs(mesh, &mut control.sub(0.3, 1.0))?;
    let first = first_new as u32;
    let new_crossings = pairs
        .iter()
        .filter(|&&(a, b)| a >= first || b >= first)
        .count();
    Ok(Verified {
        closed: status.closed,
        oriented: status.oriented,
        new_crossings,
        old_crossings: new_crossings < pairs.len(),
    })
}

// ---------------------------------------------------------------------------------------------
// Estimate

/// `ShellEstimate` in src/shell/types.ts: what a run would take, from the parts' boxes and areas.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Estimate {
    /// Grid points over all parts.
    pub points: f64,
    /// Distances measured one by one (the narrow band).
    pub measured: f64,
    /// Peak memory, MB, for the largest part.
    pub megabytes: f64,
    pub seconds: f64,
    /// Longer than a minute.
    pub slow: bool,
    /// More than 4 GB.
    pub heavy: bool,
    /// Past what a run may take: the sentence saying so.
    pub refused: Option<String>,
}

/// One part for the estimate: its world box size (mm), surface area (mm²) and triangles.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct EstimatePart {
    pub size: [f64; 3],
    pub area: f64,
    pub triangles: f64,
}

/// Seconds per distance measured and per grid point classified, on one worker thread (that work
/// spreads over all but one core); per triangle marching tetrahedra makes, which the tidying
/// reduces one collapse at a time; and per triangle of the part. Fitted to 500 Thingi10K runs
/// (brief 12 report): within a factor of two either way for four runs in five.
const SECONDS_PER_MEASURE: f64 = 4.0e-7;
const SECONDS_PER_POINT: f64 = 3.2e-8;
const SECONDS_PER_MADE: f64 = 1.6e-5;
const SECONDS_PER_TRIANGLE: f64 = 6.0e-7;

pub fn estimate(parts: &[EstimatePart], s: &ShellSettings) -> Estimate {
    let (t, h) = (s.thickness, s.accuracy);
    let grow = match s.kind.offset(t) {
        Offset::Outer(_) => 2.0 * (t + 2.0 * h),
        Offset::Inner(_) => 0.0,
    };
    let threads = crate::workers::count() as f64;
    let mut out = Estimate {
        points: 0.0,
        measured: 0.0,
        megabytes: 0.0,
        seconds: 0.0,
        slow: false,
        heavy: false,
        refused: None,
    };
    if settings_problem(s).is_some() {
        return out;
    }
    for p in parts {
        let points: f64 = p
            .size
            .iter()
            .map(|&x| ((x.max(0.0) + grow) / h + 5.0).ceil())
            .product();
        // The band: blocks within reach of the offset surface, about two blocks thick.
        let cells = p.area.max(0.0) / (h * h);
        let measured =
            cells * (4 * field::BLOCK) as f64 * if s.kind.improved() { 1.3 } else { 1.0 };
        let made = 2.5 * cells + if s.kind.hollow() { p.triangles } else { 0.0 };
        // Two bits a grid point (inside and solid), and per band cell, part triangle and triangle
        // made what the peaks of the corpus runs come to (at most a tenth of them above it).
        let bytes = points / 4.0 + cells * 480.0 + p.triangles * 640.0 + made * 800.0;
        out.points += points;
        out.measured += measured;
        out.megabytes = out.megabytes.max(bytes / 1e6);
        out.seconds += (measured * SECONDS_PER_MEASURE + points * SECONDS_PER_POINT) / threads
            + 2.5 * cells * SECONDS_PER_MADE
            + p.triangles * SECONDS_PER_TRIANGLE;
        if points > MAX_POINTS && out.refused.is_none() {
            out.refused = Some(too_fine(h));
        }
    }
    out.slow = out.seconds > 60.0;
    out.heavy = out.megabytes > 4000.0;
    out
}

#[cfg(test)]
mod tests;
