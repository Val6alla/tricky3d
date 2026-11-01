//! Refine: splits triangles until no edge is longer than the limit. It repeats Repair's edge split
//! (src/repair/refine.rs `split_edges`: every edge longer than the limit at its midpoint, and
//! every triangle on it, so no T-junction appears) until none is left. New points lie on the old
//! edges, so the shape does not change. With Selected triangles only, only edges of selected
//! triangles (and of the triangles split out of them) are split; their unselected neighbours are
//! split along the shared edges only.

use serde::Deserialize;

use super::{Modified, mask, triangles};
use crate::mesh::Mesh;
use crate::repair::refine::split_edges;
use crate::repair::topology::{Edges, NONE, side_ends};
use crate::repair::{Control, RepairError, RepairResult};

/// `RefineSettings` in src/modify/types.ts.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RefineSettings {
    /// mm.
    pub max_edge: f64,
    pub selected_only: bool,
}

/// Most triangles a refinement may make.
pub const MAX_TRIANGLES: usize = 40_000_000;

impl RefineSettings {
    pub fn problem(&self) -> Option<String> {
        (!(self.max_edge.is_finite() && self.max_edge > 0.0))
            .then(|| "Enter a Max. edge length greater than 0 mm.".into())
    }
}

/// How many triangles refining `mesh` would make, about: each chosen triangle into pieces with
/// sides of the limit (an equilateral one of side L into (side / L)² pieces).
fn estimate(mesh: &Mesh, chosen: Option<&[bool]>, limit: f64) -> f64 {
    let piece = 3f64.sqrt() / 4.0 * limit * limit;
    mesh.indices
        .as_chunks::<3>()
        .0
        .iter()
        .enumerate()
        .map(|(t, c)| {
            if chosen.is_some_and(|m| !m[t]) {
                return 1.0;
            }
            let [a, b, c] = c.map(|v| crate::repair::exact::vertex(mesh, v));
            let area = crate::repair::exact::length(crate::repair::exact::cross(
                crate::repair::exact::sub(b, a),
                crate::repair::exact::sub(c, a),
            )) / 2.0;
            (area / piece).max(1.0)
        })
        .sum()
}

pub fn refine(
    mesh: &Mesh,
    selection: &[u32],
    s: &RefineSettings,
    control: &mut Control,
) -> RepairResult<Modified> {
    let n = mesh.triangle_count();
    let chosen0 = s.selected_only.then(|| mask(selection, n));
    if chosen0.as_ref().is_some_and(|c| !c.contains(&true)) {
        return Err(RepairError::Failed(
            "Select the triangles to refine first, or switch off Selected triangles only.".into(),
        ));
    }
    if estimate(mesh, chosen0.as_deref(), s.max_edge) > MAX_TRIANGLES as f64 {
        return Err(RepairError::Failed(format!(
            "Refining to {} mm would make more than {} triangles. Enter a larger Max. edge \
             length.",
            s.max_edge,
            crate::mesh::fmt_count(MAX_TRIANGLES as u64)
        )));
    }
    control.stage("Splitting edges");
    let mut current = mesh.clone();
    // The input triangle of each triangle of `current`.
    let mut origin: Vec<u32> = (0..n as u32).collect();
    let mut chosen = chosen0;
    let limit2 = s.max_edge * s.max_edge;
    let mut rounds = 0;
    loop {
        control.check()?;
        let edges = Edges::new(&current);
        let mut split = vec![false; edges.len()];
        let mut any = false;
        for e in 0..edges.len() as u32 {
            let (a, b) = side_ends(&current, edges.sides(e)[0]);
            let (pa, pb) = (current.vertex(a), current.vertex(b));
            let d: f64 = (0..3).map(|k| (pa[k] as f64 - pb[k] as f64).powi(2)).sum();
            if d <= limit2 {
                continue;
            }
            let picked = match &chosen {
                None => true,
                Some(m) => edges
                    .sides(e)
                    .iter()
                    .any(|&side| side != NONE && m[(side / 3) as usize]),
            };
            if picked {
                split[e as usize] = true;
                any = true;
            }
        }
        if !any {
            break;
        }
        let total = current.triangle_count();
        let Some(out) = split_edges(&current, &edges, &split, &mut control.sub(0.0, 1.0))? else {
            break;
        };
        if out.mesh.triangle_count() > MAX_TRIANGLES {
            return Err(RepairError::Failed(format!(
                "Refining would make more than {} triangles. Enter a larger Max. edge length.",
                crate::mesh::fmt_count(MAX_TRIANGLES as u64)
            )));
        }
        chosen = chosen.map(|m| out.origin.iter().map(|&t| m[t as usize]).collect());
        origin = out.origin.iter().map(|&t| origin[t as usize]).collect();
        current = out.mesh;
        rounds += 1;
        control.progress(1.0 - 1.0 / (rounds as f64 + 1.0));
        let _ = total;
    }
    control.progress(1.0);
    let made = current.triangle_count();
    if made == n {
        return Ok(Modified::unchanged(
            mesh,
            format!("No edge is longer than {} mm.", s.max_edge),
        ));
    }
    Ok(Modified {
        message: format!("Refined {} into {}.", triangles(n), triangles(made)),
        mesh: current,
        changed: true,
        origin: Some(origin),
        warnings: Vec::new(),
    })
}
