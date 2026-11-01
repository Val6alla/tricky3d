//! Remesh: the part's surface rebuilt with new triangles.
//! - Volumetric: the solid sampled on a grid (1.75 times the target edge length, which makes edges
//!   about that long on average) and its boundary built by
//!   marching tetrahedra, each point where its grid edge meets the surface (Wrap part surface's
//!   last resort, src/repair/levelset.rs `remesh`); Fast mode puts each point halfway along its
//!   grid edge instead, which is quicker and a little off the surface. Closed parts only.
//! - Manifold: the same grid build round everything within half an edge length of the surface
//!   (Generate shell's outer offset, src/ops/shell.rs), so open sheets and holes come out as a
//!   closed, manifold skin about an edge length thick. The result lies that half length outside.
//! - Uniform and Adaptive: the surface kept, its triangles evened out by repeating, five times:
//!   split edges longer than 4/3 of the target, collapse edges shorter than 4/5 of it (with the
//!   collapse checks of src/ops/collapse.rs), flip edges where that brings points nearer six
//!   neighbours, and move each point towards the middle of its neighbours along the surface, then
//!   back onto the original surface (Botsch and Kobbelt). Adaptive sets each point's target from
//!   the curvature there, between the small and the large edge length. Edges sharper than the
//!   angle stay: their points only move along them, corners stay put. A last pass removes slim
//!   triangles by flipping or collapsing. With Selected triangles only, points outside the
//!   selection and its border keep their place and their triangles.

use std::collections::HashSet;

use rayon::prelude::*;
use serde::Deserialize;

use super::{Modified, Soundness, inside, mask, on_open_edges, triangles};
use crate::mesh::Mesh;
use crate::ops::collapse::{self, Plan, Quadric, Rounds, Rules, Scratch, Work, normal, rounded};
use crate::ops::shell::field::{Offset, Surface};
use crate::repair::bvh;
use crate::repair::exact::{self, P3, add, cross, dot, scale, sub};
use crate::repair::levelset::{self, CLEAR, CLEAR_MIN, Grid, MIN_CELL};
use crate::repair::{Control, NEW, RepairError, RepairResult};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Method {
    Volumetric,
    Uniform,
    Adaptive,
    Manifold,
}

/// `RemeshSettings` in src/modify/types.ts. Lengths in mm, angles in degrees.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemeshSettings {
    pub method: Method,
    /// Target edge length; Adaptive's small edge length.
    pub edge: f64,
    /// Adaptive's large edge length.
    pub large_edge: f64,
    /// Uniform and Adaptive: 0..1, how far points may leave the original surface to even it out.
    pub smoothing: f64,
    pub keep_edges: bool,
    pub edge_angle: f64,
    /// Volumetric: leave the points halfway along their grid edges.
    pub fast: bool,
    pub selected_only: bool,
}

/// Most grid points sampled (as levelset).
const MAX_POINTS: f64 = 2.0e9;
/// Marching tetrahedra makes edges about 0.57 of the grid step long on average, so the grid step
/// for a target edge length is this many times it.
const CELL_PER_EDGE: f64 = 1.75;
/// Rounds of split, collapse, flip and relax.
const ROUNDS: usize = 5;
/// Smallest angle the last pass removes triangles under, degrees.
pub const SLIM_ANGLE: f64 = 15.0;

impl RemeshSettings {
    pub fn problem(&self) -> Option<String> {
        if !(self.edge.is_finite() && self.edge >= MIN_CELL) {
            return Some(format!("Enter an edge length of at least {MIN_CELL} mm."));
        }
        if self.method == Method::Adaptive
            && !(self.large_edge.is_finite() && self.large_edge >= self.edge)
        {
            return Some("Enter a large edge length at least as long as the small one.".into());
        }
        if !(self.smoothing.is_finite() && (0.0..=1.0).contains(&self.smoothing)) {
            return Some("Enter a smoothing from 0 to 1.".into());
        }
        if !(self.edge_angle.is_finite() && (0.0..=180.0).contains(&self.edge_angle)) {
            return Some("Enter an edge angle between 0° and 180°.".into());
        }
        None
    }
}

pub fn remesh(
    mesh: &Mesh,
    selection: &[u32],
    s: &RemeshSettings,
    before: Soundness,
    control: &mut Control,
) -> RepairResult<Modified> {
    let n = mesh.triangle_count();
    match s.method {
        Method::Volumetric | Method::Manifold => {
            if s.selected_only {
                return Err(RepairError::Failed(
                    "Volumetric and Manifold remeshing rebuild the whole part. Switch off \
                     Selected triangles only."
                        .into(),
                ));
            }
            let made = if s.method == Method::Volumetric {
                if !before.closed {
                    return Err(RepairError::Failed(
                        "Volumetric remeshing needs a closed part, and this one has holes. Close \
                         them first, or use Uniform or Manifold remeshing."
                            .into(),
                    ));
                }
                volumetric(mesh, s, control)?
            } else {
                manifold(mesh, s, control)?
            };
            let Some(made) = made else {
                return Err(RepairError::Failed(
                    "Nothing was left of the part at this edge length: it is thinner than the \
                     grid. Enter a smaller edge length."
                        .into(),
                ));
            };
            let count = made.triangle_count();
            Ok(Modified {
                message: format!("Remeshed {} into {}.", triangles(n), triangles(count)),
                origin: Some(vec![NEW; count]),
                mesh: made,
                changed: true,
                warnings: Vec::new(),
            })
        }
        Method::Uniform | Method::Adaptive => {
            let chosen = s.selected_only.then(|| mask(selection, n));
            if chosen.as_ref().is_some_and(|c| !c.contains(&true)) {
                return Err(RepairError::Failed(
                    "Select the triangles to remesh first, or switch off Selected triangles only."
                        .into(),
                ));
            }
            let (made, kept) = isotropic(mesh, chosen.as_deref(), s, before, control)?;
            let count = made.triangle_count();
            let mut warnings = Vec::new();
            if kept > 0 {
                warnings.push(format!(
                    "{} {} close to another part of the surface (thin walls, narrow gaps) kept                      their triangles, so the result cannot cross itself there.",
                    crate::mesh::fmt_count(kept as u64),
                    if kept == 1 { "point" } else { "points" }
                ));
            }
            Ok(Modified {
                message: format!("Remeshed {} into {}.", triangles(n), triangles(count)),
                origin: Some(vec![NEW; count]),
                mesh: made,
                changed: true,
                warnings,
            })
        }
    }
}

fn mesh_bounds(mesh: &Mesh) -> Option<bvh::Aabb> {
    let mut b = bvh::EMPTY;
    for v in 0..mesh.vertex_count() as u32 {
        bvh::include(&mut b, exact::vertex(mesh, v));
    }
    (mesh.triangle_count() > 0 && b[0][0] <= b[1][0]).then_some(b)
}

fn grid_points(bounds: &bvh::Aabb, cell: f64) -> f64 {
    (0..3)
        .map(|a| ((bounds[1][a] - bounds[0][a]) / cell + 5.0).ceil())
        .product()
}

fn too_fine(cell: f64) -> RepairError {
    RepairError::Failed(format!(
        "An edge length of {cell} mm is too fine for a part this size. Enter a larger value."
    ))
}

// ---------------------------------------------------------------------------------------------
// Volumetric and Manifold

fn volumetric(
    mesh: &Mesh,
    s: &RemeshSettings,
    control: &mut Control,
) -> RepairResult<Option<Mesh>> {
    let Some(bounds) = mesh_bounds(mesh) else {
        return Ok(None);
    };
    let cell = CELL_PER_EDGE * s.edge;
    if grid_points(&bounds, cell) > MAX_POINTS {
        return Err(too_fine(s.edge));
    }
    control.stage("Building the surface");
    if !s.fast {
        return Ok(levelset::remesh(mesh, None, cell, control)?.map(|r| r.mesh));
    }
    let points: Vec<P3> = (0..mesh.vertex_count() as u32)
        .map(|v| exact::vertex(mesh, v))
        .collect();
    let tris: Vec<[u32; 3]> = mesh.indices.as_chunks::<3>().0.to_vec();
    let grid = Grid::new(&bounds, cell);
    let inside = grid.classify(&points, &tris, None, control.cancel_flag());
    control.check()?;
    let place = |key: u64| {
        let [a, b] = grid.ends(key);
        let (p, q) = (grid.point(a), grid.point(b));
        let d = sub(q, p);
        let clear = (CLEAR / exact::length(d)).clamp(CLEAR_MIN, 0.5);
        let t = 0.5f64.clamp(clear, 1.0 - clear);
        add(p, scale(d, t))
    };
    levelset::surface(&grid, &inside, &mut control.sub(0.3, 1.0), place)
}

/// The boundary of the part's inside and everything within half an edge length of its surface.
/// Each point lies where its grid edge, from where it leaves the part, reaches that distance.
fn manifold(mesh: &Mesh, s: &RemeshSettings, control: &mut Control) -> RepairResult<Option<Mesh>> {
    let Some(bounds) = mesh_bounds(mesh) else {
        return Ok(None);
    };
    let cell = CELL_PER_EDGE * s.edge;
    let d = 0.5 * s.edge;
    let grow = d + 2.0 * cell;
    let bounds = [bounds[0].map(|x| x - grow), bounds[1].map(|x| x + grow)];
    if grid_points(&bounds, cell) > MAX_POINTS {
        return Err(too_fine(s.edge));
    }
    control.stage("Finding the inside");
    let surface = Surface::new(mesh);
    let grid = Grid::new(&bounds, cell);
    let inside = grid.classify(
        surface.points(),
        surface.triangles(),
        None,
        control.cancel_flag(),
    );
    control.check()?;
    control.progress(0.2);
    control.stage("Measuring distances");
    let solid = crate::ops::shell::field::solid(
        &grid,
        &inside,
        &surface,
        Offset::Outer(d),
        &mut control.sub(0.2, 0.7),
    )?;
    control.stage("Building the surface");
    let place = |key: u64| {
        let [a, b] = grid.ends(key);
        let (inner, outer) = if grid.at(&solid, a) { (a, b) } else { (b, a) };
        let (p, q) = (grid.point(inner), grid.point(outer));
        let dir = sub(q, p);
        // From where the edge leaves the part (the inner end, when that is outside it).
        let from = if grid.at(&inside, inner) {
            surface.last_hit(p, q).unwrap_or(0.0)
        } else {
            0.0
        };
        let at = |t: f64| add(p, scale(dir, t));
        let gap = |t: f64| surface.distance(at(t), d + 2.0 * cell) - d;
        let (mut lo, mut hi) = (from, 1.0);
        let t = if gap(lo) <= 0.0 && gap(hi) > 0.0 {
            for _ in 0..30 {
                let mid = 0.5 * (lo + hi);
                if gap(mid) <= 0.0 {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            0.5 * (lo + hi)
        } else {
            0.5
        };
        let clear = (CLEAR / exact::length(dir)).clamp(CLEAR_MIN, 0.5);
        at(t.clamp(clear, 1.0 - clear))
    };
    levelset::surface(&grid, &solid, &mut control.sub(0.7, 1.0), place)
}

// ---------------------------------------------------------------------------------------------
// Uniform and Adaptive

#[derive(Clone, Copy, Debug, PartialEq)]
enum Class {
    Free,
    /// On a sharp edge: moves along it only.
    Crease,
    /// Where sharp edges meet, or end: stays.
    Corner,
}

struct Iso<'a> {
    work: Work,
    surface: &'a Surface,
    /// Each point's edge length aimed for.
    target: Vec<f64>,
    class: Vec<Class>,
    /// Outside the selection, or on a hole's border: never moves, keeps its triangles.
    locked: Vec<bool>,
    /// Sharp edges, smaller end first.
    features: HashSet<(u32, u32)>,
    smoothing: f64,
}

fn key(a: u32, b: u32) -> (u32, u32) {
    (a.min(b), a.max(b))
}

/// Uniform or Adaptive remeshing, and how many points it left as they were. Points with another
/// sheet of the surface within two edge lengths along their normal (a thin wall, a narrow gap)
/// keep their triangles from the start, as Generate shell's tidying leaves them; where the result
/// still has triangles crossing (and the part had none), it runs again with the original points
/// round them left as they were too, up to three times.
fn isotropic(
    mesh: &Mesh,
    chosen: Option<&[bool]>,
    s: &RemeshSettings,
    before: Soundness,
    control: &mut Control,
) -> RepairResult<(Mesh, usize)> {
    let surface = Surface::new(mesh);
    let reach = 2.0
        * if s.method == Method::Adaptive {
            s.large_edge
        } else {
            s.edge
        };
    let mut held = collapse::near_other_sheets(mesh, reach, control.cancel_flag());
    control.check()?;
    let kept = held.iter().filter(|&&h| h).count();
    let mut made = None;
    for attempt in 0..TRIES {
        let span = 1.0 / TRIES as f64;
        let mut part = control.sub(attempt as f64 * span, (attempt + 1) as f64 * span);
        let out = isotropic_once(mesh, &surface, chosen, s, &held, &mut part.sub(0.0, 0.9))?;
        if before.crossings > 0 {
            return Ok((out, kept));
        }
        part.stage("Checking the result");
        let pairs = crate::repair::tritri::intersecting_pairs(&out, &mut part.sub(0.9, 1.0))?;
        if pairs.is_empty() {
            return Ok((out, kept));
        }
        let size = pairs
            .iter()
            .flat_map(|&(a, b)| [a, b])
            .map(|t| collapse::longest_side(&exact::corners(&out, t)))
            .fold(0.0f64, f64::max);
        collapse::lock_round(mesh, &out, &pairs, 2.0 * size, &mut held);
        made = Some(out);
    }
    // Still crossing: `modify::run` refuses it with its sentence.
    Ok((made.unwrap_or_else(|| mesh.clone()), kept))
}

/// How many times Uniform and Adaptive remeshing, and Smoothing, may start again round crossings.
pub(crate) const TRIES: usize = 3;

fn isotropic_once(
    mesh: &Mesh,
    surface: &Surface,
    chosen: Option<&[bool]>,
    s: &RemeshSettings,
    held: &[bool],
    control: &mut Control,
) -> RepairResult<Mesh> {
    let mut work = Work::new(mesh, f64::INFINITY);
    work.relaxed = true;
    let movable = inside(mesh, chosen);
    let open = on_open_edges(mesh);
    let locked: Vec<bool> = (0..mesh.vertex_count())
        .map(|v| !movable[v] || open[v] || held[v])
        .collect();
    // Sharp edges and the points' classes.
    let mut features = HashSet::new();
    if s.keep_edges {
        let cos = s.edge_angle.to_radians().cos();
        for (u, v) in work.edges().into_iter().map(|[u, v]| (u, v)) {
            let ts: Vec<u32> = work.around[u as usize]
                .iter()
                .copied()
                .filter(|&t| work.tris[t as usize].contains(&v))
                .collect();
            if let [a, b] = ts[..] {
                let (na, nb) = (normal(&work.corners(a)), normal(&work.corners(b)));
                let (la, lb) = (exact::length(na), exact::length(nb));
                if la == 0.0 || lb == 0.0 || dot(na, nb) < cos * la * lb {
                    features.insert(key(u, v));
                }
            }
        }
    }
    let mut count = vec![0u8; work.pos.len()];
    for &(a, b) in &features {
        count[a as usize] = count[a as usize].saturating_add(1);
        count[b as usize] = count[b as usize].saturating_add(1);
    }
    let class: Vec<Class> = count
        .iter()
        .map(|&c| match c {
            0 => Class::Free,
            2 => Class::Crease,
            _ => Class::Corner,
        })
        .collect();
    let target = match s.method {
        Method::Adaptive => adaptive_targets(&work, s.edge, s.large_edge),
        _ => vec![s.edge; work.pos.len()],
    };
    let mut iso = Iso {
        work,
        surface,
        target,
        class,
        locked,
        features,
        smoothing: s.smoothing,
    };
    for round in 0..ROUNDS {
        let span = 1.0 / (ROUNDS + 1) as f64;
        let mut part = control.sub(round as f64 * span, (round + 1) as f64 * span);
        part.stage("Remeshing");
        iso.split_long();
        part.check()?;
        part.progress(0.25);
        iso.collapse_short(&mut part.sub(0.25, 0.5))?;
        iso.flip_edges();
        part.check()?;
        part.progress(0.75);
        iso.relax(part.cancel_flag());
        part.check()?;
    }
    control.stage("Removing slim triangles");
    iso.remove_slim(
        SLIM_ANGLE,
        &mut control.sub(ROUNDS as f64 / (ROUNDS + 1) as f64, 1.0),
    )?;
    Ok(iso.work.finish())
}

/// Each point's target: as long as keeps the chord within a twentieth of the small length of a
/// circle of the point's curvature, between the small and the large length.
fn adaptive_targets(work: &Work, small: f64, large: f64) -> Vec<f64> {
    let error = small / 20.0;
    let mut ring = Vec::new();
    let mut out: Vec<f64> = (0..work.pos.len() as u32)
        .map(|v| {
            let n = work.around[v as usize]
                .iter()
                .fold([0.0; 3], |m, &t| add(m, normal(&work.corners(t))));
            let l = exact::length(n);
            if l == 0.0 {
                return large;
            }
            let n = scale(n, 1.0 / l);
            work.neighbours(v, &mut ring);
            let p = work.pos[v as usize];
            let k = ring
                .iter()
                .map(|&w| {
                    let d = sub(work.pos[w as usize], p);
                    let dd = dot(d, d);
                    if dd > 0.0 {
                        2.0 * dot(n, d).abs() / dd
                    } else {
                        0.0
                    }
                })
                .fold(0.0f64, f64::max);
            if k <= 0.0 {
                large
            } else {
                (8.0 * error / k).sqrt().clamp(small, large)
            }
        })
        .collect();
    // One averaging pass so the targets change gradually.
    let copy = out.clone();
    for (v, slot) in out.iter_mut().enumerate() {
        work.neighbours(v as u32, &mut ring);
        if !ring.is_empty() {
            let mean = ring.iter().map(|&w| copy[w as usize]).sum::<f64>() / ring.len() as f64;
            *slot = slot.min(0.5 * (*slot + mean));
        }
    }
    out
}

impl Iso<'_> {
    fn edge_target(&self, u: u32, v: u32) -> f64 {
        0.5 * (self.target[u as usize] + self.target[v as usize])
    }

    fn length(&self, u: u32, v: u32) -> f64 {
        exact::length(sub(self.work.pos[u as usize], self.work.pos[v as usize]))
    }

    /// The alive triangles with both u and v.
    fn edge_triangles(&self, u: u32, v: u32) -> Vec<u32> {
        self.work.around[u as usize]
            .iter()
            .copied()
            .filter(|&t| self.work.tris[t as usize].contains(&v))
            .collect()
    }

    fn set_tri(&mut self, t: u32, new: [u32; 3]) {
        let old = self.work.tris[t as usize];
        for v in old {
            if !new.contains(&v) {
                self.work.around[v as usize].retain(|&x| x != t);
            }
        }
        for v in new {
            if !old.contains(&v) {
                self.work.around[v as usize].push(t);
            }
        }
        self.work.tris[t as usize] = new;
    }

    fn push_tri(&mut self, tri: [u32; 3]) {
        let t = self.work.tris.len() as u32;
        self.work.tris.push(tri);
        self.work.alive.push(true);
        self.work.live += 1;
        for v in tri {
            self.work.around[v as usize].push(t);
        }
    }

    /// Splits every edge longer than 4/3 of its target at its middle (one of its ends must be free
    /// to change).
    fn split_long(&mut self) {
        for [u, v] in self.work.edges() {
            if self.locked[u as usize] && self.locked[v as usize] {
                continue;
            }
            if self.length(u, v) <= 4.0 / 3.0 * self.edge_target(u, v) {
                continue;
            }
            let ts = self.edge_triangles(u, v);
            if ts.len() != 2 {
                continue;
            }
            let p = rounded(scale(
                add(self.work.pos[u as usize], self.work.pos[v as usize]),
                0.5,
            ));
            let m = self.work.pos.len() as u32;
            self.work.pos.push(p);
            self.work.around.push(Vec::new());
            let sharp = self.features.remove(&key(u, v));
            if sharp {
                self.features.insert(key(u, m));
                self.features.insert(key(m, v));
            }
            self.class
                .push(if sharp { Class::Crease } else { Class::Free });
            self.locked.push(false);
            self.target.push(self.edge_target(u, v));
            for t in ts {
                let tri = self.work.tris[t as usize];
                let k = (0..3)
                    .find(|&k| {
                        let (a, b) = (tri[k], tri[(k + 1) % 3]);
                        (a == u && b == v) || (a == v && b == u)
                    })
                    .unwrap_or(0);
                let (a, b, c) = (tri[k], tri[(k + 1) % 3], tri[(k + 2) % 3]);
                self.set_tri(t, [a, m, c]);
                self.push_tri([m, b, c]);
            }
        }
    }

    fn collapse_short(&mut self, control: &mut Control) -> RepairResult<()> {
        let mut q = vec![Quadric::default(); self.work.pos.len()];
        let features = std::mem::take(&mut self.features);
        let mut rules = ShortRules {
            target: &self.target,
            class: &self.class,
            locked: &self.locked,
            features,
        };
        collapse::collapse_rounds(
            &mut self.work,
            &mut q,
            &mut rules,
            Rounds {
                ordered: true,
                target: 0,
            },
            control,
        )?;
        self.features = rules.features;
        Ok(())
    }

    /// Flips each edge whose flip brings its four points nearer six neighbours (four on a hole's
    /// border), where the two triangles are nearly flat and the flip folds nothing.
    fn flip_edges(&mut self) {
        let valence_aim = |iso: &Iso, v: u32| if iso.locked[v as usize] { 4 } else { 6 } as i64;
        for [u, v] in self.work.edges() {
            if self.features.contains(&key(u, v))
                || (self.locked[u as usize] && self.locked[v as usize])
            {
                continue;
            }
            let Some((t1, t2, c, d)) = self.quad(u, v) else {
                continue;
            };
            let val = |x: u32| self.work.around[x as usize].len() as i64;
            let (vu, vv, vc, vd) = (val(u), val(v), val(c), val(d));
            if vu <= 3 || vv <= 3 {
                continue;
            }
            let dev = |a: i64, x: u32| (a - valence_aim(self, x)).abs();
            let before = dev(vu, u) + dev(vv, v) + dev(vc, c) + dev(vd, d);
            let after = dev(vu - 1, u) + dev(vv - 1, v) + dev(vc + 1, c) + dev(vd + 1, d);
            if after < before && self.flip_fits(u, v, c, d) {
                self.set_tri(t1, [c, u, d]);
                self.set_tri(t2, [d, v, c]);
            }
        }
    }

    /// The two triangles of edge u–v as (u, v, c) and (v, u, d), when the flip to c–d is possible.
    fn quad(&self, u: u32, v: u32) -> Option<(u32, u32, u32, u32)> {
        let ts = self.edge_triangles(u, v);
        let [a, b] = ts[..] else {
            return None;
        };
        let third = |t: u32| {
            self.work.tris[t as usize]
                .into_iter()
                .find(|&x| x != u && x != v)
        };
        let runs_uv = |t: u32| {
            let tri = self.work.tris[t as usize];
            (0..3).any(|k| tri[k] == u && tri[(k + 1) % 3] == v)
        };
        let (t1, t2) = if runs_uv(a) { (a, b) } else { (b, a) };
        if !runs_uv(t1) || runs_uv(t2) {
            return None;
        }
        let (c, d) = (third(t1)?, third(t2)?);
        if c == d || !self.edge_triangles(c, d).is_empty() {
            return None;
        }
        Some((t1, t2, c, d))
    }

    /// The flipped triangles are no slivers, face the way the old pair did, and the old pair was
    /// nearly flat (so the flip does not cut across a ridge).
    fn flip_fits(&self, u: u32, v: u32, c: u32, d: u32) -> bool {
        let p = |x: u32| self.work.pos[x as usize];
        let old = [[p(u), p(v), p(c)], [p(v), p(u), p(d)]];
        let new = [[p(c), p(u), p(d)], [p(d), p(v), p(c)]];
        let (n0, n1) = (normal(&old[0]), normal(&old[1]));
        let (l0, l1) = (exact::length(n0), exact::length(n1));
        if l0 == 0.0 || l1 == 0.0 || dot(n0, n1) < 0.94 * l0 * l1 {
            return false;
        }
        let mean = add(scale(n0, 1.0 / l0), scale(n1, 1.0 / l1));
        new.iter().all(|t| {
            let n = normal(t);
            let l = exact::length(n);
            l > 0.0 && dot(n, mean) > 0.5 * l * exact::length(mean) && collapse::quality(t) > 0.05
        })
    }

    /// Moves each free point towards the middle of its neighbours along the surface (and, with
    /// smoothing, a little across it), each point on a sharp edge along that edge, then back onto
    /// the original surface. A move that would fold a triangle is left out.
    fn relax(&mut self, cancel: &std::sync::atomic::AtomicBool) {
        let w: &Iso = self;
        let targets: Vec<Option<P3>> = (0..w.work.pos.len() as u32)
            .into_par_iter()
            .map_init(Vec::new, |ring, v| {
                if cancel.load(std::sync::atomic::Ordering::Relaxed)
                    || w.locked[v as usize]
                    || w.work.around[v as usize].is_empty()
                {
                    return None;
                }
                let p = w.work.pos[v as usize];
                match w.class[v as usize] {
                    Class::Corner => None,
                    Class::Crease => {
                        w.work.neighbours(v, ring);
                        let along: Vec<u32> = ring
                            .iter()
                            .copied()
                            .filter(|&x| w.features.contains(&key(v, x)))
                            .collect();
                        let [a, b] = along[..] else {
                            return None;
                        };
                        let q = scale(add(w.work.pos[a as usize], w.work.pos[b as usize]), 0.5);
                        let n = w.work.around[v as usize]
                            .iter()
                            .fold([0.0; 3], |m, &t| add(m, normal(&w.work.corners(t))));
                        Some(rounded(w.onto(add(p, scale(sub(q, p), 0.5)), n)))
                    }
                    Class::Free => {
                        w.work.neighbours(v, ring);
                        if ring.is_empty() {
                            return None;
                        }
                        let q = scale(
                            ring.iter()
                                .fold([0.0; 3], |m, &x| add(m, w.work.pos[x as usize])),
                            1.0 / ring.len() as f64,
                        );
                        let n = w.work.around[v as usize]
                            .iter()
                            .fold([0.0; 3], |m, &t| add(m, normal(&w.work.corners(t))));
                        let l = exact::length(n);
                        if l == 0.0 {
                            return None;
                        }
                        let n = scale(n, 1.0 / l);
                        let d = sub(q, p);
                        let across = scale(n, dot(d, n));
                        let along = sub(d, across);
                        let on = w.onto(add(p, along), n);
                        Some(rounded(add(on, scale(across, w.smoothing))))
                    }
                }
            })
            .collect();
        for (v, x) in targets.into_iter().enumerate() {
            if let Some(x) = x {
                collapse::try_move(&mut self.work, v as u32, x);
            }
        }
    }

    /// The nearest point of the original surface facing the way `n` does (in a thin wall the
    /// far side is nearer to some points than their own side), else the nearest point.
    fn onto(&self, p: P3, n: P3) -> P3 {
        self.surface
            .nearest_facing(p, n, f64::INFINITY)
            .or_else(|| self.surface.nearest(p, f64::INFINITY).map(|(_, q)| q))
            .unwrap_or(p)
    }

    /// Flips or collapses away triangles with an angle under `degrees`, a few passes over them.
    fn remove_slim(&mut self, degrees: f64, control: &mut Control) -> RepairResult<()> {
        for _ in 0..3 {
            control.check()?;
            let slim: Vec<u32> = (0..self.work.tris.len() as u32)
                .filter(|&t| {
                    self.work.alive[t as usize] && smallest_angle(&self.work.corners(t)) < degrees
                })
                .collect();
            if slim.is_empty() {
                break;
            }
            let mut changed = false;
            for t in slim {
                if !self.work.alive[t as usize] || smallest_angle(&self.work.corners(t)) >= degrees
                {
                    continue;
                }
                let tri = self.work.tris[t as usize];
                // Its longest edge, flipped; else its shortest, collapsed.
                let sides: Vec<(f64, u32, u32)> = (0..3)
                    .map(|k| {
                        (
                            self.length(tri[k], tri[(k + 1) % 3]),
                            tri[k],
                            tri[(k + 1) % 3],
                        )
                    })
                    .collect();
                let longest = sides
                    .iter()
                    .copied()
                    .fold(sides[0], |m, s| if s.0 > m.0 { s } else { m });
                let (_, u, v) = longest;
                if !self.features.contains(&key(u, v))
                    && !(self.locked[u as usize] && self.locked[v as usize])
                    && let Some((t1, t2, c, d)) = self.quad(u, v)
                    && self.flip_fits(u, v, c, d)
                    && self.min_angle_of(&[[c, u, d], [d, v, c]])
                        > self.min_angle_of(&[
                            self.work.tris[t1 as usize],
                            self.work.tris[t2 as usize],
                        ])
                {
                    self.set_tri(t1, [c, u, d]);
                    self.set_tri(t2, [d, v, c]);
                    changed = true;
                    continue;
                }
                let shortest = sides
                    .iter()
                    .copied()
                    .fold(sides[0], |m, s| if s.0 < m.0 { s } else { m });
                let (_, a, b) = shortest;
                let rules = ShortRules {
                    target: &self.target,
                    class: &self.class,
                    locked: &self.locked,
                    features: std::mem::take(&mut self.features),
                };
                let mut k = Scratch::default();
                let plan = rules.slim_plan(&self.work, a, b, &mut k);
                self.features = rules.features;
                if let Some(p) = plan
                    && collapse::linked(&self.work, p.u, p.v, &mut k)
                {
                    remap_features(&mut self.features, &self.work, p.u, p.v);
                    let shared = k.shared;
                    collapse::collapse(&mut self.work, p.u, p.v, p.x, &shared);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        control.progress(1.0);
        Ok(())
    }

    fn min_angle_of(&self, tris: &[[u32; 3]]) -> f64 {
        tris.iter()
            .map(|t| smallest_angle(&t.map(|v| self.work.pos[v as usize])))
            .fold(f64::INFINITY, f64::min)
    }
}

/// The smallest angle of a triangle, degrees (0 without area).
pub fn smallest_angle(c: &[P3; 3]) -> f64 {
    let mut least = f64::INFINITY;
    for k in 0..3 {
        let (a, b) = (sub(c[(k + 1) % 3], c[k]), sub(c[(k + 2) % 3], c[k]));
        let (la, lb) = (exact::length(a), exact::length(b));
        if la == 0.0 || lb == 0.0 {
            return 0.0;
        }
        let angle = exact::length(cross(a, b)).atan2(dot(a, b)).to_degrees();
        least = least.min(angle);
    }
    least
}

/// After u collapses into v: u's sharp edges become v's.
fn remap_features(features: &mut HashSet<(u32, u32)>, work: &Work, u: u32, v: u32) {
    let mut ring = Vec::new();
    work.neighbours(u, &mut ring);
    for w in ring {
        if features.remove(&key(u, w)) && w != v {
            features.insert(key(v, w));
        }
    }
}

/// Collapsing edges shorter than 4/5 of their target (Uniform and Adaptive).
struct ShortRules<'a> {
    target: &'a [f64],
    class: &'a [Class],
    locked: &'a [bool],
    features: HashSet<(u32, u32)>,
}

impl ShortRules<'_> {
    /// Which end moves (the freer one) and where to: (mover, keeper, position), if either may.
    fn direction(&self, work: &Work, a: u32, b: u32) -> Option<(u32, u32, P3)> {
        let free = |x: u32| !self.locked[x as usize];
        let class = |x: u32| self.class[x as usize];
        let sharp = self.features.contains(&key(a, b));
        let may_move = |m: u32, k: u32| {
            free(m)
                && match class(m) {
                    Class::Corner => false,
                    // Along its own sharp edge only, onto another point of it.
                    Class::Crease => sharp && class(k) != Class::Free,
                    Class::Free => true,
                }
        };
        let (pa, pb) = (work.pos[a as usize], work.pos[b as usize]);
        match (may_move(a, b), may_move(b, a)) {
            (true, true) if class(a) == Class::Free && class(b) == Class::Free => {
                Some((a, b, rounded(scale(add(pa, pb), 0.5))))
            }
            (true, _) => Some((a, b, pb)),
            (false, true) => Some((b, a, pa)),
            _ => None,
        }
    }

    /// The collapse of a linked edge, if its star keeps its shape and no edge grows past 4/3 of
    /// its target.
    fn check(
        &self,
        work: &Work,
        (u, v, x): (u32, u32, P3),
        k: &mut Scratch,
        stretch: f64,
    ) -> Option<Plan> {
        if !collapse::linked(work, u, v, k) || !collapse::shape_fits(work, u, v, x, k) {
            return None;
        }
        let long = k.star.iter().any(|&t| {
            let tri = work.tris[t as usize].map(|w| if w == u { v } else { w });
            (0..3).any(|i| {
                let (a, b) = (tri[i], tri[(i + 1) % 3]);
                let pa = if a == v { x } else { work.pos[a as usize] };
                let pb = if b == v { x } else { work.pos[b as usize] };
                let aim = 0.5 * (self.target[a as usize] + self.target[b as usize]);
                exact::length(sub(pa, pb)) > stretch * aim
            })
        });
        (!long).then_some(Plan { u, v, x })
    }

    /// A slim triangle's shortest edge collapsed, allowing edges up to twice their target.
    fn slim_plan(&self, work: &Work, a: u32, b: u32, k: &mut Scratch) -> Option<Plan> {
        let d = self.direction(work, a, b)?;
        self.check(work, d, k, 2.0)
    }
}

impl Rules for ShortRules<'_> {
    fn price(&self, work: &Work, _q: &[Quadric], u: u32, v: u32) -> Option<f64> {
        let len = exact::length(sub(work.pos[u as usize], work.pos[v as usize]));
        let aim = 0.5 * (self.target[u as usize] + self.target[v as usize]);
        (len < 0.8 * aim && self.direction(work, u, v).is_some()).then_some(len)
    }

    fn plan(&self, work: &Work, _q: &[Quadric], u: u32, v: u32, k: &mut Scratch) -> Option<Plan> {
        let d = self.direction(work, u, v)?;
        self.check(work, d, k, 4.0 / 3.0)
    }

    fn before_apply(&mut self, work: &Work, plans: &[Plan]) {
        for p in plans {
            remap_features(&mut self.features, work, p.u, p.v);
        }
    }
}
