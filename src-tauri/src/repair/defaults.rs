//! Every built-in value of repair mode in one place: the action catalog (labels, parameters, units,
//! defaults), the three built-in scripts, and the type numbers of the reference tool's `.repair`
//! files. The built-in scripts must match the reference tool exactly: same actions, same order,
//! same parameter values.

use serde::Serialize;
use serde_json::{Map, Value, json};

use super::scripts::{Script, Step};
use super::{
    ActionId, DegenerateParams, SanitizeParams, StitchParams, TinyShellMode, TinyShellParams,
};

/// Sanitize duplicate nodes: mm.
pub const SANITIZE: SanitizeParams = SanitizeParams { tolerance: 0.0001 };

/// Stitch triangles when a script step gives no values.
pub const STITCH: StitchParams = StitchParams {
    tolerance: 0.010,
    ignore_orientation: false,
};

/// Remove degenerate faces when a script step gives no values; the Status tab's degenerate count
/// and highlight use the same threshold.
pub const DEGENERATE: DegenerateParams = DegenerateParams {
    calc_tolerance: true,
    tolerance: 0.0020,
};

/// Remove tiny shells when a script step gives no values.
pub const TINY_SHELLS: TinyShellParams = TinyShellParams {
    mode: TinyShellMode::ClosedVolumeOpenArea,
    min_volume: 0.0001,
    min_area: 0.0001,
};

/// The mm a Remove degenerate faces tolerance stands for: the tolerance itself, with "Calculate
/// tolerance by bounding box" on or off. Calibrated against the reference tool's own Extended
/// repair results (its built-in step has the option on): the shortest edge of every one of them
/// is the 0.002 mm tolerance, for parts from 3 to 780 mm across, so the option scales nothing at
/// these sizes. Scaling by the bounding-box diagonal, longest side or volume, or by the mean edge
/// length, left edges below 0.002 mm in our results that the reference tool's never have.
pub fn effective_degenerate_tolerance(params: &DegenerateParams) -> f64 {
    params.tolerance
}

// ---------------------------------------------------------------------------------------------
// Catalog (`Catalog` in src/repair/types.ts)

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamKind {
    Float,
    Bool,
    Choice,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    pub id: &'static str,
    pub label: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamSpec {
    /// The key in a script step's `params`.
    pub id: &'static str,
    pub label: &'static str,
    pub kind: ParamKind,
    /// "mm", "mm³" or "mm²" for numbers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<&'static str>,
    /// Decimals the editor shows, so 0.01 reads "0.010" and 0.002 reads "0.0020".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u32>,
    /// Smallest value allowed (numbers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    pub default: Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<Choice>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionSpec {
    pub id: ActionId,
    pub label: &'static str,
    /// False: listed with a Soon badge and cannot be added or run.
    pub implemented: bool,
    /// Listed in the Repair scripts tab's add-action dropdown.
    pub in_scripts: bool,
    /// Needs a triangle selection (Close defined holes, Remove/Flip selected triangles).
    pub needs_selection: bool,
    /// The action's type number in the reference tool's `.repair` files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repair_type: Option<u32>,
    /// One sentence for tooltips.
    pub hint: &'static str,
    pub params: Vec<ParamSpec>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    /// In add-action dropdown order (alphabetical, like the reference tool), then the actions
    /// that only the Actions tab and the ribbon offer.
    pub actions: Vec<ActionSpec>,
    /// The built-in scripts, in picker order.
    pub scripts: Vec<Script>,
}

fn mm(id: &'static str, label: &'static str, decimals: u32, default: f64) -> ParamSpec {
    ParamSpec {
        id,
        label,
        kind: ParamKind::Float,
        unit: Some("mm"),
        decimals: Some(decimals),
        min: Some(0.0),
        default: json!(default),
        choices: Vec::new(),
    }
}

fn unit(mut p: ParamSpec, unit: &'static str) -> ParamSpec {
    p.unit = Some(unit);
    p
}

fn yes_no(id: &'static str, label: &'static str, default: bool) -> ParamSpec {
    ParamSpec {
        id,
        label,
        kind: ParamKind::Bool,
        unit: None,
        decimals: None,
        min: None,
        default: json!(default),
        choices: Vec::new(),
    }
}

pub const TINY_SHELL_MODES: [(TinyShellMode, &str, &str); 3] = [
    (
        TinyShellMode::ClosedVolumeOpenArea,
        "closed_volume_open_area",
        "Delete closed shells below minimum volume and open shells below minimum area",
    ),
    (
        TinyShellMode::Volume,
        "volume",
        "Delete shells below minimum volume",
    ),
    (
        TinyShellMode::Area,
        "area",
        "Delete shells below minimum area",
    ),
];

/// Parameter specs of an action, in editor order.
pub fn params_of(id: ActionId) -> Vec<ParamSpec> {
    match id {
        ActionId::SanitizeDuplicateNodes => {
            vec![mm("tolerance", "Tolerance", 4, SANITIZE.tolerance)]
        }
        ActionId::StitchTriangles => vec![
            mm("tolerance", "Tolerance", 3, STITCH.tolerance),
            yes_no(
                "ignore_orientation",
                "Ignore orientation",
                STITCH.ignore_orientation,
            ),
        ],
        ActionId::RemoveDegenerateFaces => vec![
            yes_no(
                "calc_tolerance",
                "Calculate tolerance by bounding box",
                DEGENERATE.calc_tolerance,
            ),
            mm("tolerance", "Tolerance", 4, DEGENERATE.tolerance),
        ],
        ActionId::RemoveTinyShells => vec![
            ParamSpec {
                id: "mode",
                label: "Tiny-shell detection",
                kind: ParamKind::Choice,
                unit: None,
                decimals: None,
                min: None,
                default: json!(TINY_SHELL_MODES[0].1),
                choices: TINY_SHELL_MODES
                    .iter()
                    .map(|&(_, id, label)| Choice { id, label })
                    .collect(),
            },
            unit(
                mm("min_volume", "Minimum volume", 4, TINY_SHELLS.min_volume),
                "mm³",
            ),
            unit(
                mm("min_area", "Minimum area", 4, TINY_SHELLS.min_area),
                "mm²",
            ),
        ],
        _ => Vec::new(),
    }
}

/// (id, label, implemented, in scripts, needs selection, .repair type, hint).
type Entry = (
    ActionId,
    &'static str,
    bool,
    bool,
    bool,
    Option<u32>,
    &'static str,
);

/// The catalog, in order.
const ACTIONS: [Entry; 36] = [
    (
        ActionId::CloseAllHoles,
        "Close all holes",
        true,
        true,
        false,
        Some(4),
        "Fill every hole: flat holes with flat patches, curved holes with patches that follow the surface.",
    ),
    (
        ActionId::CloseTrivialHoles,
        "Close trivial holes",
        true,
        true,
        false,
        Some(3),
        "Fill holes bordered by 3 edges, or by 4 edges that are nearly flat.",
    ),
    (
        ActionId::FixFlippedTriangles,
        "Fix flipped triangles",
        true,
        true,
        false,
        Some(2),
        "Turn triangles that face the wrong way, then make each closed shell face outward.",
    ),
    (
        ActionId::InvertNegativeShells,
        "Invert negative shells",
        true,
        true,
        false,
        None,
        "Turn closed shells that are inside out (negative volume).",
    ),
    (
        ActionId::PrefixSuffix,
        "Prefix-Suffix",
        false,
        true,
        false,
        None,
        "Rename the part with a prefix or suffix.",
    ),
    (
        ActionId::ReduceTriangles,
        "Reduce triangles",
        false,
        true,
        false,
        None,
        "Use fewer triangles while keeping the shape.",
    ),
    (
        ActionId::RefineTriangles,
        "Refine triangles",
        false,
        true,
        false,
        None,
        "Split large triangles into smaller ones.",
    ),
    (
        ActionId::Remesh,
        "Remesh",
        false,
        true,
        false,
        None,
        "Rebuild the surface with evenly sized triangles.",
    ),
    (
        ActionId::RemoveDegenerateFaces,
        "Remove degenerate faces",
        true,
        true,
        false,
        Some(6),
        "Collapse triangles with two corners closer than the tolerance, and remove those without area.",
    ),
    (
        ActionId::RemoveDoubleTriangles,
        "Remove double triangles",
        true,
        true,
        false,
        Some(5),
        "Delete triangles that repeat another triangle's three corners.",
    ),
    (
        ActionId::RemoveProblemAreas,
        "Remove problem areas",
        true,
        true,
        false,
        None,
        "Delete degenerate, self-intersecting and non-manifold triangles so their holes can be closed again.",
    ),
    (
        ActionId::RemoveSelfIntersections,
        "Remove self-intersections",
        true,
        true,
        false,
        None,
        "Delete triangles that cut through other triangles, leaving holes to close.",
    ),
    (
        ActionId::RemoveTinyShells,
        "Remove tiny shells",
        true,
        true,
        false,
        Some(7),
        "Delete shells below a size threshold.",
    ),
    (
        ActionId::SanitizeDuplicateNodes,
        "Sanitize duplicate nodes",
        true,
        true,
        false,
        None,
        "Merge corners that lie closer together than the tolerance.",
    ),
    (
        ActionId::Scale,
        "Scale",
        false,
        true,
        false,
        None,
        "Resize the part.",
    ),
    (
        ActionId::SmoothTriangles,
        "Smooth triangles",
        true,
        true,
        false,
        None,
        "Move each point inside the selection, or of the whole part, to the average of its neighbours.",
    ),
    (
        ActionId::SplitNonManifoldEdges,
        "Split non-manifold edges",
        true,
        true,
        false,
        None,
        "Separate edges shared by more than two triangles.",
    ),
    (
        ActionId::SplitOffSelfIntersections,
        "Split-off self-intersections",
        true,
        true,
        false,
        None,
        "Move triangles that cut through others into a shell of their own.",
    ),
    (
        ActionId::StitchTriangles,
        "Stitch triangles",
        true,
        true,
        false,
        Some(1),
        "Join open edges that lie within the tolerance, closing gaps and cracks.",
    ),
    (
        ActionId::WrapPartSurface,
        "Wrap part surface",
        true,
        true,
        false,
        Some(13),
        "Keep only the outer surface: remove inner shells, inner faces and self-intersections.",
    ),
    (
        ActionId::ZCompensation,
        "Z-compensation",
        false,
        true,
        false,
        None,
        "Shorten the part in Z to make up for layer growth.",
    ),
    (
        ActionId::CloseDefinedHoles,
        "Close defined holes",
        true,
        false,
        true,
        None,
        "Fill the holes that border the selected triangles.",
    ),
    (
        ActionId::SplitNonOrientedEdges,
        "Split non-oriented edges",
        true,
        false,
        false,
        None,
        "Separate edges whose two triangles face opposite ways.",
    ),
    (
        ActionId::DetectSelfIntersections,
        "Detect self-intersections",
        true,
        false,
        false,
        None,
        "Find and highlight triangles that cut through other triangles.",
    ),
    (
        ActionId::RemoveSelectedTriangles,
        "Remove selected triangles",
        true,
        false,
        true,
        None,
        "Delete the selected triangles.",
    ),
    (
        ActionId::FlipSelectedTriangles,
        "Flip selected triangles",
        true,
        false,
        true,
        None,
        "Turn the selected triangles to face the other way.",
    ),
    (
        ActionId::CloseHolesCurved,
        "Close holes along the curvature",
        true,
        false,
        false,
        None,
        "Fill holes with patches that carry on the curve of the surface round them, flat holes too.",
    ),
    (
        ActionId::BridgeGap,
        "Bridge a gap",
        true,
        false,
        false,
        None,
        "Join the nearest two opposing open edges with two triangles; with none to join, stitch open edges.",
    ),
    (
        ActionId::GrowHoleBand,
        "Grow a band from a hole edge",
        true,
        false,
        false,
        None,
        "Add a band of triangles along each open loop.",
    ),
    (
        ActionId::SubdivideTriangles,
        "Subdivide triangles",
        true,
        false,
        false,
        None,
        "Split each triangle, or each selected one, into four.",
    ),
    (
        ActionId::AddTriangle,
        "Add triangle",
        true,
        false,
        false,
        None,
        "Fill each three-edge hole with a triangle; with none, close trivial holes.",
    ),
    (
        ActionId::AddPoint,
        "Add point",
        true,
        false,
        true,
        None,
        "Add a point in the middle of each selected triangle.",
    ),
    (
        ActionId::MoveNode,
        "Move nodes",
        true,
        false,
        false,
        None,
        "Drag a point of the surface to a new place; the triangles round it follow.",
    ),
    (
        ActionId::CutSurface,
        "Cut surfaces",
        true,
        false,
        false,
        None,
        "Cut the surface open along a line drawn on it through clicked points.",
    ),
    (
        ActionId::SelectProblemTriangles,
        "Select by criteria",
        true,
        false,
        false,
        None,
        "Select every damaged triangle: open, non-manifold, slim, inverted or degenerate.",
    ),
    (
        ActionId::FindOverlaps,
        "Find overlapping triangles",
        true,
        false,
        false,
        None,
        "Select triangles that cross or repeat.",
    ),
];

pub fn label_of(id: ActionId) -> &'static str {
    ACTIONS
        .iter()
        .find(|a| a.0 == id)
        .map_or("Repair action", |a| a.1)
}

pub fn is_implemented(id: ActionId) -> bool {
    ACTIONS.iter().any(|a| a.0 == id && a.2)
}

pub fn repair_type_of(id: ActionId) -> Option<u32> {
    ACTIONS.iter().find(|a| a.0 == id).and_then(|a| a.5)
}

/// The action a `.repair` file's type number stands for.
pub fn action_of_repair_type(number: u32) -> Option<ActionId> {
    ACTIONS.iter().find(|a| a.5 == Some(number)).map(|a| a.0)
}

pub fn catalog() -> Catalog {
    Catalog {
        actions: ACTIONS
            .iter()
            .map(
                |&(id, label, implemented, in_scripts, needs_selection, repair_type, hint)| {
                    ActionSpec {
                        id,
                        label,
                        implemented,
                        in_scripts,
                        needs_selection,
                        repair_type,
                        hint,
                        params: params_of(id),
                    }
                },
            )
            .collect(),
        scripts: builtin_scripts(),
    }
}

// ---------------------------------------------------------------------------------------------
// Built-in scripts

pub const SIMPLE_ID: &str = "builtin.simple";
pub const DEFAULT_ID: &str = "builtin.default";
pub const EXTENDED_ID: &str = "builtin.extended";

fn step(action: ActionId, params: Value) -> Step {
    let params: Map<String, Value> = match params {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    Step {
        action,
        enabled: true,
        params,
    }
}

fn plain(action: ActionId) -> Step {
    step(action, json!({}))
}

fn stitch(ignore_orientation: bool) -> Step {
    step(
        ActionId::StitchTriangles,
        json!({ "tolerance": 0.010, "ignore_orientation": ignore_orientation }),
    )
}

/// Default repair's ten steps; Extended repair inserts Wrap part surface after the eighth.
fn default_steps() -> Vec<Step> {
    vec![
        stitch(false),
        plain(ActionId::CloseTrivialHoles),
        plain(ActionId::RemoveDoubleTriangles),
        plain(ActionId::FixFlippedTriangles),
        plain(ActionId::CloseTrivialHoles),
        stitch(true),
        plain(ActionId::FixFlippedTriangles),
        plain(ActionId::CloseAllHoles),
        step(
            ActionId::RemoveDegenerateFaces,
            json!({ "calc_tolerance": true, "tolerance": 0.0020 }),
        ),
        step(
            ActionId::RemoveTinyShells,
            json!({
                "mode": "closed_volume_open_area",
                "min_volume": 0.0001,
                "min_area": 0.0001,
            }),
        ),
    ]
}

/// Simple, Default and Extended repair, in picker order.
pub fn builtin_scripts() -> Vec<Script> {
    let simple = vec![
        plain(ActionId::FixFlippedTriangles),
        plain(ActionId::CloseTrivialHoles),
        plain(ActionId::CloseAllHoles),
    ];
    let mut extended = default_steps();
    extended.insert(8, plain(ActionId::WrapPartSurface));
    vec![
        Script {
            id: SIMPLE_ID.into(),
            name: "Simple repair".into(),
            built_in: true,
            actions: simple,
        },
        Script {
            id: DEFAULT_ID.into(),
            name: "Default repair".into(),
            built_in: true,
            actions: default_steps(),
        },
        Script {
            id: EXTENDED_ID.into(),
            name: "Extended repair".into(),
            built_in: true,
            actions: extended,
        },
    ]
}

pub fn builtin_script(id: &str) -> Option<Script> {
    builtin_scripts().into_iter().find(|s| s.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(script: &Script) -> Vec<ActionId> {
        script.actions.iter().map(|s| s.action).collect()
    }

    #[test]
    fn builtin_scripts_match_the_reference_tool() {
        use ActionId::*;
        let scripts = builtin_scripts();
        assert_eq!(
            scripts.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["Simple repair", "Default repair", "Extended repair"]
        );
        assert_eq!(
            ids(&scripts[0]),
            [FixFlippedTriangles, CloseTrivialHoles, CloseAllHoles]
        );
        let default = [
            StitchTriangles,
            CloseTrivialHoles,
            RemoveDoubleTriangles,
            FixFlippedTriangles,
            CloseTrivialHoles,
            StitchTriangles,
            FixFlippedTriangles,
            CloseAllHoles,
            RemoveDegenerateFaces,
            RemoveTinyShells,
        ];
        assert_eq!(ids(&scripts[1]), default);
        let mut extended = default.to_vec();
        extended.insert(8, WrapPartSurface);
        assert_eq!(ids(&scripts[2]), extended);
        assert!(scripts.iter().all(|s| s.built_in));
        assert!(scripts.iter().flat_map(|s| &s.actions).all(|a| a.enabled));

        // The two Stitch steps differ only in Ignore orientation.
        let d = &scripts[1].actions;
        assert_eq!(d[0].params["tolerance"], json!(0.010));
        assert_eq!(d[0].params["ignore_orientation"], json!(false));
        assert_eq!(d[5].params["tolerance"], json!(0.010));
        assert_eq!(d[5].params["ignore_orientation"], json!(true));
        assert_eq!(d[8].params["calc_tolerance"], json!(true));
        assert_eq!(d[8].params["tolerance"], json!(0.0020));
        assert_eq!(d[9].params["mode"], json!("closed_volume_open_area"));
        assert_eq!(d[9].params["min_volume"], json!(0.0001));
        assert_eq!(d[9].params["min_area"], json!(0.0001));
        assert_eq!(scripts[2].actions[9], d[8]);
        assert_eq!(scripts[2].actions[10], d[9]);
    }

    #[test]
    fn catalog_lists_the_add_action_dropdown_in_order() {
        let names: Vec<&str> = catalog()
            .actions
            .iter()
            .filter(|a| a.in_scripts)
            .map(|a| a.label)
            .collect();
        assert_eq!(
            names,
            [
                "Close all holes",
                "Close trivial holes",
                "Fix flipped triangles",
                "Invert negative shells",
                "Prefix-Suffix",
                "Reduce triangles",
                "Refine triangles",
                "Remesh",
                "Remove degenerate faces",
                "Remove double triangles",
                "Remove problem areas",
                "Remove self-intersections",
                "Remove tiny shells",
                "Sanitize duplicate nodes",
                "Scale",
                "Smooth triangles",
                "Split non-manifold edges",
                "Split-off self-intersections",
                "Stitch triangles",
                "Wrap part surface",
                "Z-compensation",
            ]
        );
    }

    #[test]
    fn repair_type_numbers_round_trip() {
        for (number, id) in [
            (1, ActionId::StitchTriangles),
            (2, ActionId::FixFlippedTriangles),
            (3, ActionId::CloseTrivialHoles),
            (4, ActionId::CloseAllHoles),
            (5, ActionId::RemoveDoubleTriangles),
            (6, ActionId::RemoveDegenerateFaces),
            (7, ActionId::RemoveTinyShells),
            (13, ActionId::WrapPartSurface),
        ] {
            assert_eq!(action_of_repair_type(number), Some(id));
            assert_eq!(repair_type_of(id), Some(number));
        }
        assert_eq!(action_of_repair_type(8), None);
    }

    #[test]
    fn the_tolerance_is_used_as_given() {
        assert_eq!(effective_degenerate_tolerance(&DEGENERATE), 0.002);
        let fixed = DegenerateParams {
            calc_tolerance: false,
            ..DEGENERATE
        };
        assert_eq!(effective_degenerate_tolerance(&fixed), 0.002);
    }
}
