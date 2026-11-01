//! Modify › Manipulate mesh and the other mesh tools of the Modify tab: tools that change a part's
//! triangles (Triangle reduction, Smoothing, Refine, Remesh, Z-compensation, Extrude, Offset a
//! selection, Offset hole edges, Invert). Each one is a plain function on the part's working copy
//! in world space (the window bakes the part's matrix in first, as repair does), optionally
//! limited to a triangle selection. `run` adds what every tool shares: a result that was closed,
//! oriented and free of crossing triangles before stays so, or the tool is refused and the part
//! is left as it was. Nothing here touches Tauri; src/jobs/modify.rs runs it off the UI thread.

pub mod extrude;
pub mod reduce;
pub mod refine;
pub mod remesh;
pub mod smooth;
pub mod zcomp;

use serde::Deserialize;

use crate::mesh::{Mesh, fmt_count};
use crate::repair::topology::{Edges, is_consistent};
use crate::repair::{Control, RepairError, RepairResult, status, tritri};

/// `ToolSettings` in src/modify/types.ts: the tool and its settings.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "tool", rename_all = "kebab-case")]
pub enum Tool {
    Reduce(reduce::ReduceSettings),
    Smooth(smooth::SmoothSettings),
    Refine(refine::RefineSettings),
    Remesh(remesh::RemeshSettings),
    ZCompensation(zcomp::ZCompSettings),
    Extrude(extrude::ExtrudeSettings),
    OffsetSelection(extrude::OffsetSettings),
    OffsetHoles(extrude::HoleOffsetSettings),
    /// Turn the whole part inside out.
    Invert,
    /// Turn the closed shells that are inside out (negative volume).
    InvertNegative,
}

impl Tool {
    /// The tool's name, as messages and the undo step say it.
    pub fn label(&self) -> &'static str {
        match self {
            Tool::Reduce(_) => "Triangle reduction",
            Tool::Smooth(_) => "Smoothing",
            Tool::Refine(_) => "Refine",
            Tool::Remesh(_) => "Remesh",
            Tool::ZCompensation(_) => "Z-compensation",
            Tool::Extrude(_) => "Extrude",
            Tool::OffsetSelection(_) => "Offset selection",
            Tool::OffsetHoles(_) => "Offset hole edges",
            Tool::Invert => "Invert",
            Tool::InvertNegative => "Invert negative shells",
        }
    }

    /// The sentence for settings the panel should not have let through; None when they are fine.
    pub fn problem(&self) -> Option<String> {
        match self {
            Tool::Reduce(s) => s.problem(),
            Tool::Smooth(s) => s.problem(),
            Tool::Refine(s) => s.problem(),
            Tool::Remesh(s) => s.problem(),
            Tool::ZCompensation(s) => s.problem(),
            Tool::Extrude(s) => s.problem(),
            Tool::OffsetSelection(s) => s.problem(),
            Tool::OffsetHoles(s) => s.problem(),
            Tool::Invert | Tool::InvertNegative => None,
        }
    }

    /// Turns the part inside out on purpose: its triangles then disagree with its shell's outside.
    fn inverts(&self) -> bool {
        matches!(self, Tool::Invert)
    }
}

/// A tool's result.
#[derive(Clone, Debug, PartialEq)]
pub struct Modified {
    pub mesh: Mesh,
    /// False when the mesh is exactly the input.
    pub changed: bool,
    /// For each output triangle, the input triangle it came from (`repair::NEW` for new ones), so
    /// a triangle selection can follow; None when the triangles are the input's in order.
    pub origin: Option<Vec<u32>>,
    /// What changed: "Reduced 12,000 triangles to 1,200."
    pub message: String,
    pub warnings: Vec<String>,
}

impl Modified {
    pub fn unchanged(mesh: &Mesh, message: impl Into<String>) -> Modified {
        Modified {
            mesh: mesh.clone(),
            changed: false,
            origin: None,
            message: message.into(),
            warnings: Vec::new(),
        }
    }

    /// The same triangles in the same order with vertices moved.
    pub fn moved(mesh: Mesh, message: impl Into<String>) -> Modified {
        Modified {
            mesh,
            changed: true,
            origin: None,
            message: message.into(),
            warnings: Vec::new(),
        }
    }
}

/// What the result checks look at.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Soundness {
    pub closed: bool,
    /// The Status tab's oriented: no inverted triangles, no edge whose triangles disagree.
    pub oriented: bool,
    /// No edge whose two triangles run the same way (an inside-out part is consistent).
    pub consistent: bool,
    pub crossings: usize,
}

/// Closed, oriented and crossing pairs, as Repair's Status tab and Detect self-intersections
/// tell.
pub fn soundness(mesh: &Mesh, control: &mut Control) -> RepairResult<Soundness> {
    let s = status::analyze(mesh, &mut control.sub(0.0, 0.3))?.status;
    let edges = Edges::new(mesh);
    let consistent =
        (0..edges.len() as u32).all(|e| edges.degree(e) != 2 || is_consistent(mesh, &edges, e));
    let crossings = tritri::intersecting_pairs(mesh, &mut control.sub(0.3, 1.0))?.len();
    Ok(Soundness {
        closed: s.closed,
        oriented: s.oriented,
        consistent,
        crossings,
    })
}

/// Runs `tool` on `mesh` (world space) with `selection` (sorted triangle ids; empty for none) and
/// checks the result against the input: what was closed, oriented and free of crossings stays so,
/// else the tool fails with a sentence and nothing changes.
pub fn run(
    tool: &Tool,
    mesh: &Mesh,
    selection: &[u32],
    control: &mut Control,
) -> RepairResult<Modified> {
    if let Some(problem) = tool.problem() {
        return Err(RepairError::Failed(problem));
    }
    control.stage("Checking the part");
    let before = soundness(mesh, &mut control.sub(0.0, 0.1))?;
    let mut work = control.sub(0.1, 0.9);
    let out = match tool {
        Tool::Reduce(s) => reduce::reduce(mesh, selection, s, before, &mut work)?,
        Tool::Smooth(s) => smooth::smooth(mesh, selection, s, before, &mut work)?,
        Tool::Refine(s) => refine::refine(mesh, selection, s, &mut work)?,
        Tool::Remesh(s) => remesh::remesh(mesh, selection, s, before, &mut work)?,
        Tool::ZCompensation(s) => zcomp::compensate(mesh, s, &mut work)?,
        Tool::Extrude(s) => extrude::extrude(mesh, selection, s, &mut work)?,
        Tool::OffsetSelection(s) => extrude::offset_selection(mesh, selection, s, &mut work)?,
        Tool::OffsetHoles(s) => extrude::offset_holes(mesh, selection, s, &mut work)?,
        Tool::Invert => invert(mesh),
        Tool::InvertNegative => {
            let o = crate::repair::invert::run(mesh, selection, &mut work)?;
            Modified {
                mesh: o.mesh,
                changed: o.changed,
                origin: None,
                message: o.message,
                warnings: o.warnings,
            }
        }
    };
    drop(work);
    if !out.changed {
        control.progress(1.0);
        return Ok(out);
    }
    control.stage("Checking the result");
    let after = soundness(&out.mesh, &mut control.sub(0.9, 1.0))?;
    if let Some(problem) = refusal(tool, &before, &after) {
        return Err(RepairError::Failed(problem));
    }
    control.progress(1.0);
    Ok(out)
}

/// Why the result may not replace the part, if it may not.
pub fn refusal(tool: &Tool, before: &Soundness, after: &Soundness) -> Option<String> {
    let mut what = Vec::new();
    if before.closed && !after.closed {
        what.push("open it");
    }
    let oriented = if tool.inverts() {
        before.consistent && !after.consistent
    } else {
        before.oriented && !after.oriented
    };
    if oriented {
        what.push("leave triangles facing the wrong way");
    }
    if before.crossings == 0 && after.crossings > 0 {
        what.push("make triangles cross each other");
    }
    (!what.is_empty()).then(|| {
        format!(
            "{} would {} with these settings, so the part was left as it was. {}",
            tool.label(),
            what.join(" and "),
            match tool {
                Tool::Reduce(_) => "Try a smaller Max. deformation or a higher target.",
                Tool::Smooth(_) => "Try fewer iterations or a lower strength.",
                Tool::Remesh(_) => "Try a smaller edge length.",
                Tool::Extrude(_)
                | Tool::OffsetSelection(_)
                | Tool::OffsetHoles(_)
                | Tool::ZCompensation(_) => {
                    "Try a smaller distance."
                }
                _ => "Repair the part first.",
            }
        )
    })
}

/// The part turned inside out: every triangle's second and third corners swapped.
pub fn invert(mesh: &Mesh) -> Modified {
    let mut out = mesh.clone();
    for t in out.indices.as_chunks_mut::<3>().0 {
        t.swap(1, 2);
    }
    let n = mesh.triangle_count();
    let mut m = Modified::moved(
        out,
        format!(
            "Turned the part inside out ({} {}).",
            fmt_count(n as u64),
            if n == 1 { "triangle" } else { "triangles" }
        ),
    );
    m.changed = n > 0;
    m
}

/// "1 triangle", "12,000 triangles".
pub fn triangles(n: usize) -> String {
    format!(
        "{} {}",
        fmt_count(n as u64),
        if n == 1 { "triangle" } else { "triangles" }
    )
}

/// A mask of `selection` over `count` triangles.
pub fn mask(selection: &[u32], count: usize) -> Vec<bool> {
    let mut m = vec![false; count];
    for &t in selection {
        if let Some(slot) = m.get_mut(t as usize) {
            *slot = true;
        }
    }
    m
}

/// For each vertex: every triangle round it is in `chosen` (and it has one), so the tool may move
/// it. With `chosen` None every vertex with a triangle may.
pub fn inside(mesh: &Mesh, chosen: Option<&[bool]>) -> Vec<bool> {
    let mut all = vec![true; mesh.vertex_count()];
    let mut any = vec![false; mesh.vertex_count()];
    for (t, tri) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        let picked = chosen.is_none_or(|c| c[t]);
        for &v in tri {
            any[v as usize] = true;
            if !picked {
                all[v as usize] = false;
            }
        }
    }
    all.iter().zip(&any).map(|(&a, &b)| a && b).collect()
}

/// The vertices on an edge that does not have exactly two triangles (a hole's border or a
/// non-manifold edge): tools that keep the mesh's outline leave them where they are.
pub fn on_open_edges(mesh: &Mesh) -> Vec<bool> {
    let edges = Edges::new(mesh);
    let mut out = vec![false; mesh.vertex_count()];
    for e in 0..edges.len() as u32 {
        if edges.degree(e) != 2 {
            for v in edges.ends(e) {
                out[v as usize] = true;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
