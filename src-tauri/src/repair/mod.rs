//! Repair mode: the repair actions and the scripts that chain them. Every action is a
//! plain function `(mesh, selection, params, control) -> Outcome` in a file of its own, so a script
//! is just a list of calls. Nothing here touches Tauri types; src/jobs.rs moves the work onto a
//! background thread and converts the IPC types.
//!
//! Actions work on the part's working copy in world space (the window bakes the part's matrix in
//! first), in millimetres. A selection is a sorted list of triangle indices of the input mesh.

pub mod defaults;
pub mod scripts;

// Shared building blocks.
pub mod bvh;
pub mod exact;
pub mod orient;
pub mod shells;
pub mod status;
pub mod topology;
pub mod tritri;
pub mod winding;

// One file per action.
pub mod degenerate;
pub mod doubles;
pub mod edit;
pub mod flipped;
pub mod holes;
pub mod invert;
pub mod problem_areas;
pub mod sanitize;
pub mod self_intersect;
pub mod split_edges;
pub mod stitch;
pub mod tiny_shells;
pub mod wrap;

// The Home tab's local edits and selections (Home › Repair), and Smooth triangles.
pub mod bridge;
pub mod find;
pub mod nodes;
pub mod open_edges;
pub mod refine;
pub mod smooth;
pub mod surface_cut;

// Wrap part surface's mesh arrangement, and its last resort.
pub mod arrangement;
pub mod levelset;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::mesh::Mesh;

/// Marks a triangle that an action created (no input triangle), in an origin map.
pub const NEW: u32 = u32::MAX;

// ---------------------------------------------------------------------------------------------
// Errors, cancelling and progress

#[derive(Clone, Debug, PartialEq)]
pub enum RepairError {
    /// The user cancelled; the caller keeps the mesh it had before.
    Cancelled,
    /// A full sentence for the user, such as "Wrap part surface ran out of memory.".
    Failed(String),
}

pub type RepairResult<T> = Result<T, RepairError>;

/// What a running action may ask of its caller: whether to stop, and where to report progress.
/// Actions call `check()` every few thousand iterations of any loop that can run long, and
/// `progress()` with how far along they are (0..1). An action of several long steps names the one
/// it starts with `stage()`, for the progress dialog.
pub struct Control<'a> {
    cancel: &'a AtomicBool,
    report: Box<dyn FnMut(f64) + 'a>,
    stage: Box<dyn FnMut(&'static str) + 'a>,
}

impl<'a> Control<'a> {
    pub fn new(cancel: &'a AtomicBool, report: impl FnMut(f64) + 'a) -> Control<'a> {
        Control {
            cancel,
            report: Box::new(report),
            stage: Box::new(|_| {}),
        }
    }

    /// The same control, with `stage` receiving the names of the steps the action starts.
    pub fn with_stages(self, stage: impl FnMut(&'static str) + 'a) -> Control<'a> {
        Control {
            stage: Box::new(stage),
            ..self
        }
    }

    /// The action starts the step `name` ("Finding intersections").
    pub fn stage(&mut self, name: &'static str) {
        (self.stage)(name);
    }

    /// Err(Cancelled) once the user has asked to stop.
    #[inline]
    pub fn check(&self) -> RepairResult<()> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(RepairError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// The flag behind `check()`, for work spread over threads (rayon loops) that cannot borrow the
    /// control: read it with `Ordering::Relaxed` and stop early when it is set.
    pub fn cancel_flag(&self) -> &'a AtomicBool {
        self.cancel
    }

    /// How far along the current action is, 0..1.
    pub fn progress(&mut self, fraction: f64) {
        (self.report)(fraction.clamp(0.0, 1.0));
    }

    /// A control for a step that covers `start..end` of this one's progress, for an action that
    /// runs another action inside it (Wrap closes holes first, for example).
    pub fn sub<'b>(&'b mut self, start: f64, end: f64) -> Control<'b> {
        let cancel = self.cancel;
        let report = &mut self.report;
        let stage = &mut self.stage;
        Control {
            cancel,
            report: Box::new(move |f| report(start + (end - start) * f)),
            stage: Box::new(stage),
        }
    }
}

/// A control that never cancels and reports nowhere, for tests and internal calls.
pub fn silent<R>(work: impl FnOnce(&mut Control) -> R) -> R {
    let never = AtomicBool::new(false);
    let mut control = Control::new(&never, |_| {});
    work(&mut control)
}

// ---------------------------------------------------------------------------------------------
// Results

/// Where each output triangle came from.
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    /// Same triangles in the same order (an action that only moved vertices or flipped triangles).
    Same,
    /// For each output triangle, the input triangle it came from, or `NEW`.
    Map(Vec<u32>),
}

/// One action's result.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub mesh: Mesh,
    /// False when the mesh is exactly the input (then `origin` is `Same`).
    pub changed: bool,
    pub origin: Origin,
    /// What changed, as a sentence for the log and toast: "Closed 4 holes, added 212 triangles."
    pub message: String,
    /// Things the user should know, as sentences: "2 holes could not be closed."
    pub warnings: Vec<String>,
    /// What some of `warnings` say is wrong with the result, by index into `warnings`, so a script
    /// can tell when a later action left none of it (`run_script`).
    pub concerns: Vec<(usize, Concern)>,
    /// Triangles to highlight afterwards (Detect self-intersections), as output triangle indices.
    pub highlight: Option<Vec<u32>>,
    /// The selection afterwards, as output triangle indices, for an action that sets it (Select
    /// by criteria, or an edit that leaves nothing selected). None: the selection follows its
    /// triangles through `origin`.
    pub select: Option<Vec<u32>>,
}

/// Something a warning says is wrong with an action's result, which a later action of the same
/// run can put right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Concern {
    /// Holes left open: border edges.
    Holes,
    /// Edges whose two triangles run the same way.
    Misoriented,
    /// Degenerate faces at this tolerance (mm), as Remove degenerate faces finds them.
    Degenerate(f64),
    /// Triangles that cross each other.
    Crossings,
}

impl Concern {
    /// True when `mesh` still has some of it.
    fn present(self, mesh: &Mesh, control: &mut Control) -> RepairResult<bool> {
        Ok(match self {
            Concern::Holes => {
                let edges = topology::Edges::new(mesh);
                (0..edges.len() as u32).any(|e| edges.degree(e) == 1)
            }
            Concern::Misoriented => {
                let edges = topology::Edges::new(mesh);
                (0..edges.len() as u32)
                    .any(|e| edges.degree(e) == 2 && !topology::is_consistent(mesh, &edges, e))
            }
            Concern::Degenerate(tolerance) => {
                !status::degenerate_triangles(mesh, tolerance).is_empty()
            }
            Concern::Crossings => !tritri::intersecting_pairs(mesh, control)?.is_empty(),
        })
    }

    /// The warning once the action `label` left none of it: "1 hole could not be closed. Closed
    /// by Wrap part surface."
    fn resolved(self, warning: &str, label: &str) -> String {
        let done = match self {
            Concern::Holes => "Closed",
            Concern::Misoriented => "Oriented",
            Concern::Degenerate(_) => "Removed",
            Concern::Crossings => "Resolved",
        };
        format!("{warning} {done} by {label}.")
    }
}

impl Outcome {
    /// The input unchanged, with a message such as "No holes to close.".
    pub fn unchanged(mesh: &Mesh, message: impl Into<String>) -> Outcome {
        Outcome {
            mesh: mesh.clone(),
            changed: false,
            origin: Origin::Same,
            message: message.into(),
            warnings: Vec::new(),
            concerns: Vec::new(),
            highlight: None,
            select: None,
        }
    }
}

/// The log line for one action of a run (`Report` in src/repair/types.ts).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub action: ActionId,
    pub label: String,
    pub message: String,
    /// What is still wrong once the whole run is done.
    pub warnings: Vec<String>,
    /// Warnings a later action of the run put right, reworded to say which one (`Concern`).
    pub resolved: Vec<String>,
    pub changed: bool,
    pub millis: u64,
}

// ---------------------------------------------------------------------------------------------
// Actions

/// Every action the add-action dropdown and the Actions tab know, by its stable id. The serde
/// names are the ids saved in scripts and sent by the window. The last group exists in the
/// catalog only (shown with a Soon badge); running one is an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionId {
    SanitizeDuplicateNodes,
    StitchTriangles,
    CloseTrivialHoles,
    CloseAllHoles,
    CloseDefinedHoles,
    RemoveDoubleTriangles,
    FixFlippedTriangles,
    InvertNegativeShells,
    RemoveDegenerateFaces,
    RemoveTinyShells,
    SplitNonOrientedEdges,
    SplitNonManifoldEdges,
    DetectSelfIntersections,
    SplitOffSelfIntersections,
    RemoveSelfIntersections,
    RemoveProblemAreas,
    WrapPartSurface,
    RemoveSelectedTriangles,
    FlipSelectedTriangles,
    SmoothTriangles,
    // Home › Repair only.
    CloseHolesCurved,
    BridgeGap,
    GrowHoleBand,
    SubdivideTriangles,
    AddTriangle,
    AddPoint,
    MoveNode,
    CutSurface,
    SelectProblemTriangles,
    FindOverlaps,
    // Catalog only (Soon).
    PrefixSuffix,
    ReduceTriangles,
    RefineTriangles,
    Remesh,
    Scale,
    ZCompensation,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SanitizeParams {
    /// mm; vertices closer than this become one.
    pub tolerance: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StitchParams {
    /// mm; the largest distance between open border edges that get joined.
    pub tolerance: f64,
    /// False: only join edges whose triangles would be consistently oriented afterwards.
    pub ignore_orientation: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DegenerateParams {
    /// "Calculate tolerance by bounding box", kept for the reference tool's scripts; it changes
    /// nothing (see defaults::effective_degenerate_tolerance).
    pub calc_tolerance: bool,
    /// mm; triangles whose shortest edge is below this get collapsed.
    pub tolerance: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TinyShellMode {
    /// Delete closed shells below the minimum volume and open shells below the minimum area.
    ClosedVolumeOpenArea,
    /// Delete shells below the minimum volume.
    Volume,
    /// Delete shells below the minimum area.
    Area,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TinyShellParams {
    pub mode: TinyShellMode,
    /// mm³.
    pub min_volume: f64,
    /// mm².
    pub min_area: f64,
}

/// An action with its parameters, ready to run. Built from a script step by scripts::parse_action,
/// which fills in defaults and checks the values.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    SanitizeDuplicateNodes(SanitizeParams),
    StitchTriangles(StitchParams),
    CloseTrivialHoles,
    CloseAllHoles,
    CloseDefinedHoles,
    RemoveDoubleTriangles,
    FixFlippedTriangles,
    InvertNegativeShells,
    RemoveDegenerateFaces(DegenerateParams),
    RemoveTinyShells(TinyShellParams),
    SplitNonOrientedEdges,
    SplitNonManifoldEdges,
    DetectSelfIntersections,
    SplitOffSelfIntersections,
    RemoveSelfIntersections,
    RemoveProblemAreas,
    WrapPartSurface,
    RemoveSelectedTriangles,
    FlipSelectedTriangles,
    SmoothTriangles,
    CloseHolesCurved,
    BridgeGap,
    GrowHoleBand,
    SubdivideTriangles,
    AddTriangle,
    AddPoint,
    MoveNode(nodes::NodeMove),
    CutSurface(surface_cut::CutLine),
    SelectProblemTriangles,
    FindOverlaps,
}

impl Action {
    pub fn id(&self) -> ActionId {
        match self {
            Action::SanitizeDuplicateNodes(_) => ActionId::SanitizeDuplicateNodes,
            Action::StitchTriangles(_) => ActionId::StitchTriangles,
            Action::CloseTrivialHoles => ActionId::CloseTrivialHoles,
            Action::CloseAllHoles => ActionId::CloseAllHoles,
            Action::CloseDefinedHoles => ActionId::CloseDefinedHoles,
            Action::RemoveDoubleTriangles => ActionId::RemoveDoubleTriangles,
            Action::FixFlippedTriangles => ActionId::FixFlippedTriangles,
            Action::InvertNegativeShells => ActionId::InvertNegativeShells,
            Action::RemoveDegenerateFaces(_) => ActionId::RemoveDegenerateFaces,
            Action::RemoveTinyShells(_) => ActionId::RemoveTinyShells,
            Action::SplitNonOrientedEdges => ActionId::SplitNonOrientedEdges,
            Action::SplitNonManifoldEdges => ActionId::SplitNonManifoldEdges,
            Action::DetectSelfIntersections => ActionId::DetectSelfIntersections,
            Action::SplitOffSelfIntersections => ActionId::SplitOffSelfIntersections,
            Action::RemoveSelfIntersections => ActionId::RemoveSelfIntersections,
            Action::RemoveProblemAreas => ActionId::RemoveProblemAreas,
            Action::WrapPartSurface => ActionId::WrapPartSurface,
            Action::RemoveSelectedTriangles => ActionId::RemoveSelectedTriangles,
            Action::FlipSelectedTriangles => ActionId::FlipSelectedTriangles,
            Action::SmoothTriangles => ActionId::SmoothTriangles,
            Action::CloseHolesCurved => ActionId::CloseHolesCurved,
            Action::BridgeGap => ActionId::BridgeGap,
            Action::GrowHoleBand => ActionId::GrowHoleBand,
            Action::SubdivideTriangles => ActionId::SubdivideTriangles,
            Action::AddTriangle => ActionId::AddTriangle,
            Action::AddPoint => ActionId::AddPoint,
            Action::MoveNode(_) => ActionId::MoveNode,
            Action::CutSurface(_) => ActionId::CutSurface,
            Action::SelectProblemTriangles => ActionId::SelectProblemTriangles,
            Action::FindOverlaps => ActionId::FindOverlaps,
        }
    }
}

/// Runs one action. `selection` holds sorted triangle indices of `mesh` (empty: none). Actions that
/// work "on the selection if one exists" limit themselves to it (see each action's doc); the
/// others ignore it. Scripts run with an empty selection.
pub fn run_action(
    action: &Action,
    mesh: &Mesh,
    selection: &[u32],
    control: &mut Control,
) -> RepairResult<Outcome> {
    control.check()?;
    match action {
        Action::SanitizeDuplicateNodes(p) => sanitize::run(mesh, selection, p, control),
        Action::StitchTriangles(p) => stitch::run(mesh, selection, p, control),
        Action::CloseTrivialHoles => holes::close_trivial(mesh, selection, control),
        Action::CloseAllHoles => holes::close_all(mesh, selection, control),
        Action::CloseDefinedHoles => holes::close_defined(mesh, selection, control),
        Action::RemoveDoubleTriangles => doubles::run(mesh, selection, control),
        Action::FixFlippedTriangles => flipped::run(mesh, selection, control),
        Action::InvertNegativeShells => invert::run(mesh, selection, control),
        Action::RemoveDegenerateFaces(p) => degenerate::run(mesh, selection, p, control),
        Action::RemoveTinyShells(p) => tiny_shells::run(mesh, selection, p, control),
        Action::SplitNonOrientedEdges => split_edges::split_non_oriented(mesh, selection, control),
        Action::SplitNonManifoldEdges => split_edges::split_non_manifold(mesh, selection, control),
        Action::DetectSelfIntersections => self_intersect::detect(mesh, selection, control),
        Action::SplitOffSelfIntersections => self_intersect::split_off(mesh, selection, control),
        Action::RemoveSelfIntersections => self_intersect::remove(mesh, selection, control),
        Action::RemoveProblemAreas => problem_areas::run(mesh, selection, control),
        Action::WrapPartSurface => wrap::run(mesh, selection, control),
        Action::RemoveSelectedTriangles => edit::remove_selected(mesh, selection, control),
        Action::FlipSelectedTriangles => edit::flip_selected(mesh, selection, control),
        Action::SmoothTriangles => smooth::smooth_triangles(mesh, selection, control),
        Action::CloseHolesCurved => holes::close_curved(mesh, selection, control),
        Action::BridgeGap => bridge::run(mesh, selection, control),
        Action::GrowHoleBand => open_edges::grow_band(mesh, selection, control),
        Action::SubdivideTriangles => refine::subdivide(mesh, selection, control),
        Action::AddTriangle => open_edges::add_triangle(mesh, selection, control),
        Action::AddPoint => refine::add_point(mesh, selection, control),
        Action::MoveNode(step) => nodes::move_node(mesh, step, control),
        Action::CutSurface(line) => surface_cut::cut(mesh, line, control),
        Action::SelectProblemTriangles => find::problem_triangles(mesh, control),
        Action::FindOverlaps => find::overlaps(mesh, control),
    }
}

/// A script's result: the final mesh, the selection carried through each step's origin map, and
/// one report per action that ran.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptOutcome {
    pub mesh: Mesh,
    pub changed: bool,
    pub selection: Vec<u32>,
    /// True when an action set the selection (`Outcome::select`) rather than it only following
    /// its triangles, so the window takes it even when the mesh did not change.
    pub selected: bool,
    pub reports: Vec<Report>,
    /// The highlight the last action that made one returned, if no later action changed the mesh.
    pub highlight: Option<Vec<u32>>,
}

/// Runs `actions` in order on `mesh`. `on_step(index, label)` is called before each action,
/// `progress` receives each action's own 0..1 (the caller combines them), and `on_stage(index,
/// name)` the steps an action names inside it. Stops at the first error; Cancelled leaves nothing
/// changed for the caller, who still has its input.
///
/// Warnings describe the final result: one about a `Concern` that a later action left none of,
/// with none of it back by the end, moves to its report's `resolved`, naming that action.
pub fn run_script(
    actions: &[(Action, String)],
    mesh: &Mesh,
    selection: &[u32],
    cancel: &AtomicBool,
    mut on_step: impl FnMut(usize, &str),
    mut progress: impl FnMut(usize, f64),
    mut on_stage: impl FnMut(usize, &'static str),
) -> RepairResult<ScriptOutcome> {
    let mut current = mesh.clone();
    let mut selection = selection.to_vec();
    let mut reports: Vec<Report> = Vec::with_capacity(actions.len());
    let mut changed = false;
    let mut selected = false;
    let mut highlight = None;
    // Warnings a later action may put right, as (report, warning, concern), and those one did, as
    // (report, warning, concern, the action that did).
    let mut open: Vec<(usize, usize, Concern)> = Vec::new();
    let mut resolved: Vec<(usize, usize, Concern, usize)> = Vec::new();
    // The last action that changed the mesh.
    let mut last_change = None;
    for (step, (action, label)) in actions.iter().enumerate() {
        on_step(step, label);
        let started = Instant::now();
        let outcome = {
            let mut control = Control::new(cancel, |f| progress(step, f))
                .with_stages(|name| on_stage(step, name));
            run_action(action, &current, &selection, &mut control)?
        };
        if let Some(chosen) = &outcome.select {
            selection.clone_from(chosen);
            selected = true;
        } else if outcome.changed {
            selection = remap_selection(&selection, &outcome.origin, outcome.mesh.triangle_count());
        }
        if outcome.changed {
            highlight = None;
        }
        if outcome.highlight.is_some() {
            highlight = outcome.highlight.clone();
        }
        reports.push(Report {
            action: action.id(),
            label: label.clone(),
            message: outcome.message,
            warnings: outcome.warnings,
            resolved: Vec::new(),
            changed: outcome.changed,
            millis: started.elapsed().as_millis() as u64,
        });
        if outcome.changed {
            changed = true;
            current = outcome.mesh;
            last_change = Some(step);
            let mut control = Control::new(cancel, |_| {});
            for c in distinct(open.iter().map(|o| o.2)) {
                if !c.present(&current, &mut control)? {
                    resolved.extend(
                        open.iter()
                            .filter(|o| o.2 == c)
                            .map(|o| (o.0, o.1, c, step)),
                    );
                    open.retain(|o| o.2 != c);
                }
            }
        }
        open.extend(outcome.concerns.into_iter().map(|(w, c)| (step, w, c)));
    }
    // What a later action brought back stays a warning.
    let mut control = Control::new(cancel, |_| {});
    let mut back = Vec::new();
    for c in distinct(resolved.iter().map(|r| r.2)) {
        let since = resolved.iter().filter(|r| r.2 == c).map(|r| r.3).min();
        if since < last_change && c.present(&current, &mut control)? {
            back.push(c);
        }
    }
    resolved.retain(|r| !back.contains(&r.2));
    // Each report's highest warning first, so taking one out leaves the others where they were.
    resolved.sort_by(|a, b| (a.0, b.1).cmp(&(b.0, a.1)));
    for (report, warning, concern, by) in resolved {
        let text = reports[report].warnings.remove(warning);
        let note = concern.resolved(&text, &reports[by].label);
        reports[report].resolved.insert(0, note);
    }
    Ok(ScriptOutcome {
        mesh: current,
        changed,
        selection,
        selected,
        reports,
        highlight,
    })
}

/// The concerns in order of first appearance, each once.
fn distinct(concerns: impl Iterator<Item = Concern>) -> Vec<Concern> {
    let mut out: Vec<Concern> = Vec::new();
    for c in concerns {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// The selection after an action: the output triangles whose origin was selected.
pub fn remap_selection(selection: &[u32], origin: &Origin, triangles: usize) -> Vec<u32> {
    match origin {
        Origin::Same => selection
            .iter()
            .copied()
            .filter(|&t| (t as usize) < triangles)
            .collect(),
        Origin::Map(map) => {
            if selection.is_empty() {
                return Vec::new();
            }
            let max = selection.last().copied().unwrap_or(0) as usize;
            let mut chosen = vec![false; max + 1];
            for &t in selection {
                chosen[t as usize] = true;
            }
            map.iter()
                .enumerate()
                .filter(|&(_, &from)| {
                    from != NEW && (from as usize) <= max && chosen[from as usize]
                })
                .map(|(t, _)| t as u32)
                .collect()
        }
    }
}

/// Composes origin maps: `second` maps the output of a step whose input had origin `first`.
pub fn compose_origin(first: &Origin, second: &Origin) -> Origin {
    match (first, second) {
        (_, Origin::Same) => first.clone(),
        (Origin::Same, Origin::Map(_)) => second.clone(),
        (Origin::Map(a), Origin::Map(b)) => Origin::Map(
            b.iter()
                .map(|&t| if t == NEW { NEW } else { a[t as usize] })
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_follows_origin_maps() {
        assert_eq!(remap_selection(&[1, 3], &Origin::Same, 4), vec![1, 3]);
        assert_eq!(remap_selection(&[1, 3], &Origin::Same, 2), vec![1]);
        // Triangle 0 deleted, 1 and 3 kept (now 0 and 2), one new triangle at the end.
        let map = Origin::Map(vec![1, 2, 3, NEW]);
        assert_eq!(remap_selection(&[1, 3], &map, 4), vec![0, 2]);
        assert_eq!(remap_selection(&[], &map, 4), Vec::<u32>::new());
    }

    /// A 20 mm cube with a slit Close all holes cannot close: two distinct vertices at one point
    /// along the top edge of its +x face (the holes tests' fixture).
    fn slit_cube() -> Mesh {
        let mut m = crate::mesh::make_box([0.0; 3], [20.0; 3]);
        m.positions
            .extend_from_slice(&[20.0, 10.0, 20.0, 20.0, 10.0, 20.0]);
        let at = |m: &Mesh, t: [u32; 3]| m.indices.as_chunks::<3>().0.iter().position(|x| *x == t);
        let side = at(&m, [1, 6, 5]).unwrap();
        m.indices[side * 3..side * 3 + 3].copy_from_slice(&[1, 6, 8]);
        m.indices.extend_from_slice(&[1, 8, 5]);
        let top = at(&m, [4, 5, 6]).unwrap();
        m.indices[top * 3..top * 3 + 3].copy_from_slice(&[4, 5, 9]);
        m.indices.extend_from_slice(&[4, 9, 6]);
        m
    }

    fn script(actions: &[Action], mesh: &Mesh, selection: &[u32]) -> ScriptOutcome {
        let named: Vec<(Action, String)> = actions
            .iter()
            .map(|a| (a.clone(), defaults::label_of(a.id()).to_string()))
            .collect();
        run_script(
            &named,
            mesh,
            selection,
            &AtomicBool::new(false),
            |_, _| {},
            |_, _| {},
            |_, _| {},
        )
        .unwrap()
    }

    #[test]
    fn a_warning_a_later_action_puts_right_reads_as_resolved() {
        const HOLE: &str = "1 hole could not be closed.";
        let m = slit_cube();
        // On its own the warning stands.
        let alone = script(&[Action::CloseAllHoles], &m, &[]);
        assert_eq!(alone.reports[0].warnings, vec![HOLE.to_string()]);
        assert!(alone.reports[0].resolved.is_empty());
        // Wrap part surface closes the slit: the warning says so and is no warning any more.
        let wrapped = script(&[Action::CloseAllHoles, Action::WrapPartSurface], &m, &[]);
        let r = &wrapped.reports;
        assert!(
            r[0].warnings.is_empty() && r[1].warnings.is_empty(),
            "{r:#?}"
        );
        assert_eq!(
            r[0].resolved,
            vec![format!("{HOLE} Closed by Wrap part surface.")]
        );
        // A later action opens a hole again: the warning stands after all. The selection, a
        // triangle along the slit, keeps Close all holes on that hole.
        let on_slit = m
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .position(|t| *t == [1, 6, 8])
            .unwrap();
        let reopened = script(
            &[
                Action::CloseAllHoles,
                Action::WrapPartSurface,
                Action::RemoveSelectedTriangles,
            ],
            &m,
            &[on_slit as u32],
        );
        let r = &reopened.reports;
        assert!(r[2].changed, "{r:#?}");
        assert_eq!(r[0].warnings, vec![HOLE.to_string()], "{r:#?}");
        assert!(r[0].resolved.is_empty());
    }

    #[test]
    fn an_action_that_sets_the_selection_replaces_it() {
        // An open cube: Select by criteria picks the four triangles round the hole, whatever was
        // selected; a later action that changes the mesh carries that selection on.
        let mut m = crate::mesh::make_box([0.0; 3], [20.0; 3]);
        m.indices.drain(0..6);
        let found = script(&[Action::SelectProblemTriangles], &m, &[9]);
        assert!(!found.changed && found.selected);
        assert_eq!(found.selection.len(), 4);
        let flipped = script(
            &[
                Action::SelectProblemTriangles,
                Action::FlipSelectedTriangles,
            ],
            &m,
            &[9],
        );
        assert_eq!(flipped.selection, found.selection);
        assert_eq!(flipped.reports[1].message, "Flipped 4 triangles.");
        // An edit that leaves nothing selected; an action that sets nothing keeps following.
        let split = script(&[Action::SubdivideTriangles], &m, &[0]);
        assert!(split.changed && split.selected && split.selection.is_empty());
        let plain = script(&[Action::FlipSelectedTriangles], &m, &[0]);
        assert!(!plain.selected);
        assert_eq!(plain.selection, vec![0]);
    }

    #[test]
    fn origins_compose() {
        let a = Origin::Map(vec![2, 0, NEW]);
        let b = Origin::Map(vec![1, NEW, 2, 0]);
        assert_eq!(compose_origin(&a, &b), Origin::Map(vec![0, NEW, NEW, 2]));
        assert_eq!(compose_origin(&a, &Origin::Same), a);
        assert_eq!(compose_origin(&Origin::Same, &b), b);
    }
}
