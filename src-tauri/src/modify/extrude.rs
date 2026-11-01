//! Tools that push a selected surface: Extrude, Offset a selection and Offset hole edges.
//! - Extrude moves the selected triangles together by a distance, along their mean normal or an
//!   axis. With side walls, the selection comes loose along its border and new triangles join the
//!   moved border to where it was; without, the border's points move too and the triangles round
//!   the selection stretch to follow.
//! - Offset a selection moves each point of the selected triangles along its own normal (from the
//!   selected triangles round it) by the distance; with a transition, the points round the
//!   selection follow less and less over that width, along a cosine.
//! - Offset hole edges grows a band of new triangles from each open loop, the given width out
//!   across the hole in the plane of the triangle beside each edge (Repair's Grow a band, src/
//!   repair/open_edges.rs, with the width typed in).

use serde::Deserialize;

use super::{Modified, mask, triangles};
use crate::mesh::{Mesh, fmt_count};
use crate::repair::exact::{self, P3, add, scale};
use crate::repair::topology::Edges;
use crate::repair::{Control, NEW, RepairError, RepairResult};

/// Which way Extrude moves the selection.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    /// Along the selection's mean normal (area weighted).
    Normal,
    X,
    Y,
    Z,
}

/// `ExtrudeSettings` in src/modify/types.ts.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExtrudeSettings {
    /// mm; negative pushes in.
    pub distance: f64,
    pub direction: Direction,
    /// Add side walls (true), or stretch the triangles round the selection (false).
    pub walls: bool,
}

/// `OffsetSettings` in src/modify/types.ts.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OffsetSettings {
    /// mm; negative moves inward.
    pub distance: f64,
    /// mm of smoothed transition round the selection; 0 for none.
    pub transition: f64,
}

/// `HoleOffsetSettings` in src/modify/types.ts.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HoleOffsetSettings {
    /// mm the band reaches out across the hole.
    pub distance: f64,
    /// Only the open edges of selected triangles.
    pub selected_only: bool,
}

pub const MAX_DISTANCE: f64 = 1000.0;

fn distance_problem(d: f64, signed: bool) -> Option<String> {
    let ok = d.is_finite() && d.abs() <= MAX_DISTANCE && if signed { d != 0.0 } else { d > 0.0 };
    (!ok).then(|| {
        if signed {
            format!("Enter a distance other than 0, up to {MAX_DISTANCE} mm either way.")
        } else {
            format!("Enter a distance greater than 0 and up to {MAX_DISTANCE} mm.")
        }
    })
}

impl ExtrudeSettings {
    pub fn problem(&self) -> Option<String> {
        distance_problem(self.distance, true)
    }
}

impl OffsetSettings {
    pub fn problem(&self) -> Option<String> {
        distance_problem(self.distance, true).or_else(|| {
            (!(self.transition.is_finite() && (0.0..=MAX_DISTANCE).contains(&self.transition)))
                .then(|| format!("Enter a transition from 0 to {MAX_DISTANCE} mm."))
        })
    }
}

impl HoleOffsetSettings {
    pub fn problem(&self) -> Option<String> {
        distance_problem(self.distance, false)
    }
}

const SELECT_FIRST: &str = "Select the triangles to move first (in repair mode).";

fn unit(v: P3) -> Option<P3> {
    let l = exact::length(v);
    (l > 0.0 && l.is_finite()).then(|| scale(v, 1.0 / l))
}

fn tri_normal(mesh: &Mesh, t: &[u32; 3]) -> P3 {
    let [a, b, c] = t.map(|v| exact::vertex(mesh, v));
    exact::normal(a, b, c)
}

fn moved_point(p: P3, by: P3) -> [f32; 3] {
    [0, 1, 2].map(|k| (p[k] + by[k]) as f32)
}

// ---------------------------------------------------------------------------------------------
// Extrude

pub fn extrude(
    mesh: &Mesh,
    selection: &[u32],
    s: &ExtrudeSettings,
    control: &mut Control,
) -> RepairResult<Modified> {
    control.check()?;
    let n = mesh.triangle_count();
    let chosen = mask(selection, n);
    if !chosen.contains(&true) {
        return Err(RepairError::Failed(SELECT_FIRST.into()));
    }
    let tris = mesh.indices.as_chunks::<3>().0;
    let dir = match s.direction {
        Direction::X => [1.0, 0.0, 0.0],
        Direction::Y => [0.0, 1.0, 0.0],
        Direction::Z => [0.0, 0.0, 1.0],
        Direction::Normal => {
            let sum = tris
                .iter()
                .zip(&chosen)
                .filter(|(_, c)| **c)
                .fold([0.0; 3], |m, (t, _)| add(m, tri_normal(mesh, t)));
            unit(sum).ok_or_else(|| {
                RepairError::Failed(
                    "The selected triangles face every way, so they have no normal to move \
                     along. Pick X, Y or Z instead."
                        .into(),
                )
            })?
        }
    };
    let by = scale(dir, s.distance);
    // Points of selected triangles, and those that also belong to an unselected one (the border).
    let mut in_sel = vec![false; mesh.vertex_count()];
    let mut in_rest = vec![false; mesh.vertex_count()];
    for (t, tri) in tris.iter().enumerate() {
        for &v in tri {
            if chosen[t] {
                in_sel[v as usize] = true;
            } else {
                in_rest[v as usize] = true;
            }
        }
    }
    control.progress(0.3);
    let mut out = mesh.clone();
    let moved_count = in_sel.iter().filter(|&&b| b).count();
    if !s.walls {
        for (v, _) in in_sel.iter().enumerate().filter(|(_, s)| **s) {
            let p = moved_point(exact::vertex(mesh, v as u32), by);
            out.positions[3 * v..3 * v + 3].copy_from_slice(&p);
        }
        control.progress(1.0);
        return Ok(Modified::moved(
            out,
            format!(
                "Moved {} ({} {}) by {} mm.",
                triangles(selection.len()),
                fmt_count(moved_count as u64),
                if moved_count == 1 { "point" } else { "points" },
                s.distance
            ),
        ));
    }
    // With walls: border points get a moved copy, inner points move.
    let mut copy = vec![u32::MAX; mesh.vertex_count()];
    for v in 0..mesh.vertex_count() {
        if !in_sel[v] {
            continue;
        }
        let p = moved_point(exact::vertex(mesh, v as u32), by);
        if in_rest[v] {
            copy[v] = (out.positions.len() / 3) as u32;
            out.positions.extend_from_slice(&p);
        } else {
            out.positions[3 * v..3 * v + 3].copy_from_slice(&p);
            copy[v] = v as u32;
        }
    }
    for (t, tri) in tris.iter().enumerate() {
        if chosen[t] {
            for k in 0..3 {
                out.indices[3 * t + k] = copy[tri[k] as usize];
            }
        }
    }
    // A wall along every side of a selected triangle whose other triangle is not selected.
    let edges = Edges::new(mesh);
    let mut walls = 0;
    for e in 0..edges.len() as u32 {
        let sides = edges.sides(e);
        let picked: Vec<u32> = sides
            .iter()
            .copied()
            .filter(|&side| chosen[(side / 3) as usize])
            .collect();
        if picked.len() != 1 || sides.len() < 2 {
            continue;
        }
        let side = picked[0];
        let t = (side / 3) as usize;
        let k = (side % 3) as usize;
        let (a, b) = (tris[t][k], tris[t][(k + 1) % 3]);
        let (a2, b2) = (copy[a as usize], copy[b as usize]);
        out.indices.extend_from_slice(&[a, b, b2, a, b2, a2]);
        walls += 1;
    }
    control.progress(1.0);
    let added = out.triangle_count() - n;
    let origin: Vec<u32> = (0..n as u32)
        .chain(std::iter::repeat_n(NEW, added))
        .collect();
    Ok(Modified {
        mesh: out,
        changed: true,
        origin: Some(origin),
        message: format!(
            "Extruded {} by {} mm, with {} along {} {}.",
            triangles(selection.len()),
            s.distance,
            triangles(added),
            fmt_count(walls as u64),
            if walls == 1 { "edge" } else { "edges" }
        ),
        warnings: Vec::new(),
    })
}

// ---------------------------------------------------------------------------------------------
// Offset a selection

pub fn offset_selection(
    mesh: &Mesh,
    selection: &[u32],
    s: &OffsetSettings,
    control: &mut Control,
) -> RepairResult<Modified> {
    control.check()?;
    let n = mesh.triangle_count();
    let chosen = mask(selection, n);
    if !chosen.contains(&true) {
        return Err(RepairError::Failed(SELECT_FIRST.into()));
    }
    let tris = mesh.indices.as_chunks::<3>().0;
    let vc = mesh.vertex_count();
    // Each point's normal from the selected triangles round it, and from all of them.
    let mut sel_normal = vec![[0.0; 3]; vc];
    let mut all_normal = vec![[0.0; 3]; vc];
    let mut in_sel = vec![false; vc];
    for (t, tri) in tris.iter().enumerate() {
        let nn = tri_normal(mesh, tri);
        for &v in tri {
            all_normal[v as usize] = add(all_normal[v as usize], nn);
            if chosen[t] {
                sel_normal[v as usize] = add(sel_normal[v as usize], nn);
                in_sel[v as usize] = true;
            }
        }
    }
    control.progress(0.3);
    let mut out = mesh.clone();
    let mut moved = 0usize;
    for v in 0..vc {
        if in_sel[v]
            && let Some(dir) = unit(sel_normal[v])
        {
            let p = moved_point(exact::vertex(mesh, v as u32), scale(dir, s.distance));
            out.positions[3 * v..3 * v + 3].copy_from_slice(&p);
            moved += 1;
        }
    }
    let mut eased = 0usize;
    if s.transition > 0.0 {
        let border: Vec<P3> = (0..vc)
            .filter(|&v| in_sel[v])
            .map(|v| exact::vertex(mesh, v as u32))
            .collect();
        let grid = Hash::new(&border, s.transition);
        for v in 0..vc {
            if v % 4096 == 0 {
                control.check()?;
            }
            if in_sel[v] || all_normal[v] == [0.0; 3] {
                continue;
            }
            let p = exact::vertex(mesh, v as u32);
            let Some(r) = grid.nearest(p, s.transition) else {
                continue;
            };
            let f = 0.5 * (1.0 + (std::f64::consts::PI * r / s.transition).cos());
            if let Some(dir) = unit(all_normal[v]) {
                let q = moved_point(p, scale(dir, s.distance * f));
                out.positions[3 * v..3 * v + 3].copy_from_slice(&q);
                eased += 1;
            }
        }
    }
    control.progress(1.0);
    let mut message = format!(
        "Moved {} {} of the selection by {} mm.",
        fmt_count(moved as u64),
        if moved == 1 { "point" } else { "points" },
        s.distance
    );
    if eased > 0 {
        message.push_str(&format!(
            " {} round it followed over {} mm.",
            fmt_count(eased as u64),
            s.transition
        ));
    }
    Ok(Modified::moved(out, message))
}

/// Points in a uniform grid of cells `cell` wide, for the nearest one within that distance.
struct Hash {
    cell: f64,
    map: std::collections::HashMap<[i64; 3], Vec<P3>>,
}

impl Hash {
    fn key(p: P3, cell: f64) -> [i64; 3] {
        p.map(|x| (x / cell).floor() as i64)
    }

    fn new(points: &[P3], cell: f64) -> Hash {
        let mut map: std::collections::HashMap<[i64; 3], Vec<P3>> = Default::default();
        for &p in points {
            map.entry(Hash::key(p, cell)).or_default().push(p);
        }
        Hash { cell, map }
    }

    /// The distance to the nearest point within `limit` (at most the cell size), if any.
    fn nearest(&self, p: P3, limit: f64) -> Option<f64> {
        let k = Hash::key(p, self.cell);
        let mut best: Option<f64> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = self.map.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) else {
                        continue;
                    };
                    for &q in list {
                        let d = exact::length(exact::sub(p, q));
                        if d < limit && best.is_none_or(|b| d < b) {
                            best = Some(d);
                        }
                    }
                }
            }
        }
        best
    }
}

// ---------------------------------------------------------------------------------------------
// Offset hole edges

pub fn offset_holes(
    mesh: &Mesh,
    selection: &[u32],
    s: &HoleOffsetSettings,
    control: &mut Control,
) -> RepairResult<Modified> {
    let only: &[u32] = if s.selected_only { selection } else { &[] };
    if s.selected_only && selection.is_empty() {
        return Err(RepairError::Failed(SELECT_FIRST.into()));
    }
    let o = crate::repair::open_edges::grow_band_by(mesh, only, Some(s.distance), control)?;
    let origin = match o.origin {
        crate::repair::Origin::Map(m) => Some(m),
        crate::repair::Origin::Same => None,
    };
    Ok(Modified {
        mesh: o.mesh,
        changed: o.changed,
        origin,
        message: o.message,
        warnings: o.warnings,
    })
}
