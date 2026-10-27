//! Repair scripts: the saved form (JSON, `RepairScript` in src/repair/types.ts), turning a step into
//! a runnable Action with its parameters checked, the shape of the user's script file in the app's
//! config folder (src/jobs.rs reads and writes it), our export format, and importing `.repair`
//! files exported from the reference tool.
//!
//! A step's `params` may leave any parameter out; the catalog default (defaults.rs) fills it in.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::defaults::{self, ParamKind, ParamSpec, TINY_SHELL_MODES};
use super::nodes::NodeMove;
use super::surface_cut::{self, CutLine, CutOptions, CutPoint};
use super::{
    Action, ActionId, DegenerateParams, SanitizeParams, StitchParams, TinyShellMode,
    TinyShellParams,
};
use crate::formats::clean_name;
use crate::formats::xml::{append_text, attribute, xml_error};

/// One script: built-in (from defaults.rs, never saved) or the user's own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Script {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub built_in: bool,
    pub actions: Vec<Step>,
}

/// One action of a script with its own parameter values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    #[serde(rename = "type")]
    pub action: ActionId,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default)]
    pub params: Map<String, Value>,
}

fn enabled_by_default() -> bool {
    true
}

/// What the config folder's repair-scripts.json holds (`StoredScripts` in src/repair/types.ts):
/// the user's own scripts, and the picker order of all scripts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StoredScripts {
    pub scripts: Vec<Script>,
    /// Script ids, built-in and own, in picker order.
    #[serde(default)]
    pub order: Vec<String>,
}

/// The runnable action for a step, with missing parameters taken from the catalog defaults. An
/// error is a reason clause naming the parameter, such as "Stitch triangles: the tolerance must
/// be a number of 0 mm or more". Parameters the action does not have are ignored.
pub fn parse_action(step: &Step) -> Result<Action, String> {
    let label = defaults::label_of(step.action);
    let unavailable = || Err(format!("{label} is not available yet"));
    if !defaults::is_implemented(step.action) {
        return unavailable();
    }
    Ok(match step.action {
        ActionId::SanitizeDuplicateNodes => Action::SanitizeDuplicateNodes(SanitizeParams {
            tolerance: number(step, "tolerance")?,
        }),
        ActionId::StitchTriangles => Action::StitchTriangles(StitchParams {
            tolerance: number(step, "tolerance")?,
            ignore_orientation: flag(step, "ignore_orientation")?,
        }),
        ActionId::CloseTrivialHoles => Action::CloseTrivialHoles,
        ActionId::CloseAllHoles => Action::CloseAllHoles,
        ActionId::CloseDefinedHoles => Action::CloseDefinedHoles,
        ActionId::RemoveDoubleTriangles => Action::RemoveDoubleTriangles,
        ActionId::FixFlippedTriangles => Action::FixFlippedTriangles,
        ActionId::InvertNegativeShells => Action::InvertNegativeShells,
        ActionId::RemoveDegenerateFaces => Action::RemoveDegenerateFaces(DegenerateParams {
            calc_tolerance: flag(step, "calc_tolerance")?,
            tolerance: number(step, "tolerance")?,
        }),
        ActionId::RemoveTinyShells => Action::RemoveTinyShells(TinyShellParams {
            mode: mode(step, "mode")?,
            min_volume: number(step, "min_volume")?,
            min_area: number(step, "min_area")?,
        }),
        ActionId::SplitNonOrientedEdges => Action::SplitNonOrientedEdges,
        ActionId::SplitNonManifoldEdges => Action::SplitNonManifoldEdges,
        ActionId::DetectSelfIntersections => Action::DetectSelfIntersections,
        ActionId::SplitOffSelfIntersections => Action::SplitOffSelfIntersections,
        ActionId::RemoveSelfIntersections => Action::RemoveSelfIntersections,
        ActionId::RemoveProblemAreas => Action::RemoveProblemAreas,
        ActionId::WrapPartSurface => Action::WrapPartSurface,
        ActionId::RemoveSelectedTriangles => Action::RemoveSelectedTriangles,
        ActionId::FlipSelectedTriangles => Action::FlipSelectedTriangles,
        ActionId::SmoothTriangles => Action::SmoothTriangles,
        ActionId::CloseHolesCurved => Action::CloseHolesCurved,
        ActionId::BridgeGap => Action::BridgeGap,
        ActionId::GrowHoleBand => Action::GrowHoleBand,
        ActionId::SubdivideTriangles => Action::SubdivideTriangles,
        ActionId::AddTriangle => Action::AddTriangle,
        ActionId::AddPoint => Action::AddPoint,
        ActionId::MoveNode => Action::MoveNode(node_move(step)?),
        ActionId::CutSurface => Action::CutSurface(cut_line(step)?),
        ActionId::SelectProblemTriangles => Action::SelectProblemTriangles,
        ActionId::FindOverlaps => Action::FindOverlaps,
        ActionId::PrefixSuffix
        | ActionId::ReduceTriangles
        | ActionId::RefineTriangles
        | ActionId::Remesh
        | ActionId::Scale
        | ActionId::ZCompensation => return unavailable(),
    })
}

/// Move nodes as the window asks for it: `vertex` and where it goes, `x`, `y`, `z` (mm).
fn node_move(step: &Step) -> Result<NodeMove, String> {
    let label = defaults::label_of(step.action);
    let bad = || format!("{label}: the point to move or its new position is missing");
    let vertex = step
        .params
        .get("vertex")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(bad)?;
    let coord = |k: &str| step.params.get(k).and_then(Value::as_f64).ok_or_else(bad);
    Ok(NodeMove {
        vertex,
        to: [coord("x")?, coord("y")?, coord("z")?],
    })
}

/// Cut surfaces as the window asks for it: `points` as [x, y, z, triangle] each, and the cut
/// panel's `smooth`, `step` (mm), `snap` (mm) and `closed`; a setting left out takes its default.
fn cut_line(step: &Step) -> Result<CutLine, String> {
    let label = defaults::label_of(step.action);
    let bad = || format!("{label}: the points of the line are damaged");
    let list = step
        .params
        .get("points")
        .and_then(Value::as_array)
        .ok_or_else(bad)?;
    let mut points = Vec::with_capacity(list.len());
    for p in list {
        let [x, y, z, t] = p.as_array().map(Vec::as_slice).unwrap_or_default() else {
            return Err(bad());
        };
        let triangle = t
            .as_u64()
            .and_then(|t| u32::try_from(t).ok())
            .ok_or_else(bad)?;
        let at = [x, y, z].map(Value::as_f64);
        let [Some(x), Some(y), Some(z)] = at else {
            return Err(bad());
        };
        points.push(CutPoint {
            at: [x, y, z],
            triangle,
        });
    }
    let given = |k: &str| step.params.get(k).filter(|v| !v.is_null());
    let flag = |k: &str, default: bool| match given(k) {
        None => Ok(default),
        Some(v) => v
            .as_bool()
            .ok_or_else(|| format!("{label}: \"{k}\" must be yes or no")),
    };
    let length = |k: &str, default: f64, min: f64, name: &str| match given(k) {
        None => Ok(default),
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite() && *x >= min)
            .ok_or_else(|| format!("{label}: the {name} must be a number of {min} mm or more")),
    };
    let d = surface_cut::OPTIONS;
    Ok(CutLine {
        points,
        options: CutOptions {
            smooth: flag("smooth", d.smooth)?,
            step: length("step", d.step, surface_cut::MIN_STEP, "smooth step length")?,
            snap: length("snap", d.snap, 0.0, "snap distance")?,
            closed: flag("closed", d.closed)?,
        },
    })
}

/// The spec of the parameter `id` of `step`'s action and the step's value for it: its own, or the
/// default when the step leaves it out (or sends null).
fn lookup(step: &Step, id: &str) -> Result<(ParamSpec, Value), String> {
    let spec = defaults::params_of(step.action)
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| {
            let label = defaults::label_of(step.action);
            format!("{label}: tricky3D has no setting called \"{id}\"")
        })?;
    let value = match step.params.get(id) {
        None | Some(Value::Null) => spec.default.clone(),
        Some(value) => value.clone(),
    };
    Ok((spec, value))
}

fn number(step: &Step, id: &str) -> Result<f64, String> {
    let (spec, value) = lookup(step, id)?;
    value
        .as_f64()
        .filter(|_| valid(&spec, &value))
        .ok_or_else(|| invalid(step.action, &spec))
}

fn flag(step: &Step, id: &str) -> Result<bool, String> {
    let (spec, value) = lookup(step, id)?;
    value.as_bool().ok_or_else(|| invalid(step.action, &spec))
}

fn mode(step: &Step, id: &str) -> Result<TinyShellMode, String> {
    let (spec, value) = lookup(step, id)?;
    value
        .as_str()
        .and_then(|text| TINY_SHELL_MODES.iter().find(|m| m.1 == text))
        .map(|m| m.0)
        .ok_or_else(|| invalid(step.action, &spec))
}

/// True when `value` is usable for `spec`: a finite number at or above its minimum, a boolean, or
/// one of its choices.
fn valid(spec: &ParamSpec, value: &Value) -> bool {
    match spec.kind {
        ParamKind::Float => value
            .as_f64()
            .is_some_and(|v| v.is_finite() && v >= spec.min.unwrap_or(f64::NEG_INFINITY)),
        ParamKind::Bool => value.is_boolean(),
        ParamKind::Choice => value
            .as_str()
            .is_some_and(|text| spec.choices.iter().any(|c| c.id == text)),
    }
}

/// What a value for `spec` must be, as a reason clause naming the action and the parameter.
fn invalid(action: ActionId, spec: &ParamSpec) -> String {
    let label = defaults::label_of(action);
    let param = spec.label.to_lowercase();
    match spec.kind {
        ParamKind::Float => {
            let unit = spec.unit.map(|u| format!(" {u}")).unwrap_or_default();
            match spec.min {
                Some(min) => {
                    format!("{label}: the {param} must be a number of {min}{unit} or more")
                }
                None => format!("{label}: the {param} must be a number"),
            }
        }
        ParamKind::Bool => format!("{label}: \"{}\" must be yes or no", spec.label),
        ParamKind::Choice => {
            format!("{label}: the {param} must be one of the choices tricky3D offers")
        }
    }
}

/// Every parameter of `action`, in catalog order: the valid values of `given`, the defaults for
/// the rest. Also the parameters whose given value was not valid, with that value, so an import
/// can say it used the default instead. Parameters the action does not have are dropped.
fn complete(
    action: ActionId,
    given: &Map<String, Value>,
) -> (Map<String, Value>, Vec<(ParamSpec, Value)>) {
    let mut params = Map::new();
    let mut rejected = Vec::new();
    for spec in defaults::params_of(action) {
        let value = match given.get(spec.id) {
            None | Some(Value::Null) => spec.default.clone(),
            Some(value) if valid(&spec, value) => value.clone(),
            Some(value) => {
                rejected.push((spec.clone(), value.clone()));
                spec.default.clone()
            }
        };
        params.insert(spec.id.to_string(), value);
    }
    (params, rejected)
}

/// The script with its file's warnings, from `import_repair_xml` or `import_json`; the JSON the
/// `repair_script_import` command returns.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Imported {
    pub script: Script,
    /// Sentences naming what was skipped or replaced by a default.
    pub warnings: Vec<String>,
}

/// The name an imported script gets when its file has none.
const IMPORTED_NAME: &str = "Imported script";

/// A new id for an imported script: "own." and the time in milliseconds, later than any id this
/// run handed out before, so two imports in the same millisecond still differ.
fn fresh_id() -> String {
    static LAST: Mutex<u64> = Mutex::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *last = now.max(*last + 1);
    format!("own.{}", *last)
}

// ---------------------------------------------------------------------------------------------
// The reference tool's `.repair` files

/// Where an element sits in a `.repair` file. An option's own `<type>` (float, boolean) is not
/// needed: the catalog knows each parameter's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tag {
    List,
    Name,
    Repair,
    Type,
    Option,
    OptionId,
    OptionValue,
    Other,
}

fn tag_for(parent: Option<Tag>, local: &str) -> Tag {
    match (parent, local.to_ascii_lowercase().as_str()) {
        (None, "repairlist") => Tag::List,
        (Some(Tag::List), "name") => Tag::Name,
        (Some(Tag::List), "repair") => Tag::Repair,
        (Some(Tag::Repair), "type") => Tag::Type,
        (Some(Tag::Repair), "option") => Tag::Option,
        (Some(Tag::Option), "id") => Tag::OptionId,
        (Some(Tag::Option), "value") => Tag::OptionValue,
        _ => Tag::Other,
    }
}

/// One `<repair>` element as read, before it is mapped to an action.
#[derive(Default)]
struct RepairEntry {
    enabled: bool,
    number: Option<String>,
    /// (id, value) per `<option>`, in file order.
    options: Vec<(String, String)>,
}

#[derive(Default)]
struct ListReader {
    found: bool,
    name: Option<String>,
    entries: Vec<RepairEntry>,
    entry: RepairEntry,
    option_id: String,
    option_value: String,
}

impl ListReader {
    /// An element closed with `text` as its content.
    fn end(&mut self, tag: Tag, text: &str) {
        match tag {
            Tag::Name if self.name.is_none() => self.name = clean_name(Some(text)),
            Tag::Type => self.entry.number = Some(text.trim().to_string()),
            Tag::OptionId => self.option_id = text.trim().to_string(),
            Tag::OptionValue => self.option_value = text.trim().to_string(),
            Tag::Option => {
                let id = std::mem::take(&mut self.option_id);
                let value = std::mem::take(&mut self.option_value);
                self.entry.options.push((id, value));
            }
            Tag::Repair => self.entries.push(std::mem::take(&mut self.entry)),
            _ => {}
        }
    }
}

/// Reads the XML `.repair` format of the reference tool: `<repairlist>` with a `<name>` and one
/// `<repair enabled="0|1">` per action. Never runs anything; only maps type numbers to actions.
///
/// Each `<repair>` holds a `<type>` number (defaults::action_of_repair_type) and `<option>`
/// elements with an `<id>`, a `<type>` and a `<value>`; options map per `repair_option`. An action
/// type or option tricky3D does not know is skipped with a warning, as is an option value that
/// does not fit (the default stays). Every step gets all of its action's parameters. Whitespace
/// and line breaks (CRLF in the reference tool's files) may sit anywhere between elements.
pub fn import_repair_xml(text: &str) -> Result<Imported, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<Tag> = Vec::new();
    let mut list = ListReader::default();
    let mut content = String::new();
    loop {
        let event = reader
            .read_event()
            .map_err(|e| xml_error("repair list", &e))?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                let tag = tag_for(stack.last().copied(), e.local_name().as_ref());
                let is_start = matches!(event, Event::Start(_));
                match tag {
                    Tag::List => list.found = true,
                    Tag::Repair => {
                        list.entry = RepairEntry {
                            enabled: enabled(attribute(e, "enabled").as_deref()),
                            ..RepairEntry::default()
                        };
                    }
                    Tag::Option => {
                        list.option_id.clear();
                        list.option_value.clear();
                    }
                    Tag::Other if is_start => {
                        reader
                            .read_to_end(e.name())
                            .map_err(|e| xml_error("repair list", &e))?;
                        continue;
                    }
                    _ => {}
                }
                content.clear();
                if is_start {
                    stack.push(tag);
                } else {
                    list.end(tag, "");
                }
            }
            Event::End(_) => {
                if let Some(tag) = stack.pop() {
                    list.end(tag, &content);
                }
                content.clear();
            }
            Event::Text(_) | Event::CData(_) | Event::GeneralRef(_) => {
                append_text(&mut content, &event)
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !list.found {
        return Err("it is not a repair list (it has no <repairlist> element)".into());
    }

    let mut warnings = Vec::new();
    let mut actions = Vec::new();
    for (n, entry) in list.entries.iter().enumerate() {
        if let Some(step) = repair_step(entry, n + 1, &mut warnings) {
            actions.push(step);
        }
    }
    Ok(Imported {
        script: Script {
            id: fresh_id(),
            name: list.name.unwrap_or_else(|| IMPORTED_NAME.into()),
            built_in: false,
            actions,
        },
        warnings,
    })
}

/// The `enabled` attribute: on unless it says "0" (or "false", "no").
fn enabled(value: Option<&str>) -> bool {
    !matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "false" | "no")
    )
}

/// The step for the `n`th `<repair>` element, or None (with a warning) when it cannot be one.
fn repair_step(entry: &RepairEntry, n: usize, warnings: &mut Vec<String>) -> Option<Step> {
    let number = match entry.number.as_deref() {
        None | Some("") => {
            warnings.push(format!("Skipped action {n}, which has no type number."));
            return None;
        }
        Some(text) => match text.parse::<u32>() {
            Ok(number) => number,
            Err(_) => {
                warnings.push(format!(
                    "Skipped action {n}, whose type \"{text}\" is not a number."
                ));
                return None;
            }
        },
    };
    let Some(action) = defaults::action_of_repair_type(number) else {
        warnings.push(format!(
            "Skipped action type {number}, which tricky3D does not know."
        ));
        return None;
    };
    let label = defaults::label_of(action);
    if !defaults::is_implemented(action) {
        warnings.push(format!(
            "Skipped {label} (action {n}), which is not available yet."
        ));
        return None;
    }
    let specs = defaults::params_of(action);
    let mut given = Map::new();
    for (id, text) in &entry.options {
        // Numeric ids compare as numbers ("01" is option 1), others in any letter case.
        let id = match id.parse::<u32>() {
            Ok(number) => number.to_string(),
            Err(_) => id.to_ascii_lowercase(),
        };
        let spec = repair_option(action, &id).and_then(|key| specs.iter().find(|s| s.id == key));
        let Some(spec) = spec else {
            warnings.push(if id.is_empty() {
                format!("Skipped an option without an id of {label} (action {n}).")
            } else {
                format!(
                    "Skipped option {id} of {label} (action {n}), which tricky3D does not know."
                )
            });
            continue;
        };
        // A choice without a value (Remove tiny shells' mode, as the reference tool writes it)
        // means the default.
        if text.is_empty() && matches!(spec.kind, ParamKind::Choice) {
            continue;
        }
        match option_value(spec, text) {
            Some(value) => {
                given.insert(spec.id.to_string(), value);
            }
            None => warnings.push(format!(
                "Used the default {} for {label} (action {n}): \"{text}\" is not valid.",
                spec.label.to_lowercase()
            )),
        }
    }
    let (params, _) = complete(action, &given);
    Some(Step {
        action,
        enabled: entry.enabled,
        params,
    })
}

/// The parameter a `.repair` option id stands for in `action` (the design's table of the reference
/// tool's ids).
fn repair_option(action: ActionId, id: &str) -> Option<&'static str> {
    Some(match (action, id) {
        (ActionId::StitchTriangles, "1") => "tolerance",
        (ActionId::StitchTriangles, "2") => "ignore_orientation",
        (ActionId::RemoveDegenerateFaces, "calc_tolerance") => "calc_tolerance",
        (ActionId::RemoveDegenerateFaces, "3") => "tolerance",
        (ActionId::RemoveTinyShells, "mode") => "mode",
        (ActionId::RemoveTinyShells, "4") => "min_volume",
        (ActionId::RemoveTinyShells, "minarea") => "min_area",
        _ => return None,
    })
}

/// An option's text as a value for `spec`, or None when it does not fit: a finite number at or
/// above the minimum (a lone comma counts as the decimal point, as a European number format writes
/// it); "1", "0", "true" or "false" (any case) for a yes/no; a choice's id.
fn option_value(spec: &ParamSpec, text: &str) -> Option<Value> {
    let value = match spec.kind {
        // A number that is not finite becomes null, which fails the check below.
        ParamKind::Float => {
            let number = if text.contains('.') || text.matches(',').count() != 1 {
                text.parse::<f64>()
            } else {
                text.replace(',', ".").parse::<f64>()
            };
            json!(number.ok()?)
        }
        ParamKind::Bool => match text.to_ascii_lowercase().as_str() {
            "1" | "true" => json!(true),
            "0" | "false" => json!(false),
            _ => return None,
        },
        ParamKind::Choice => json!(text),
    };
    valid(spec, &value).then_some(value)
}

// ---------------------------------------------------------------------------------------------
// Our own JSON

/// The `format` field of our export format.
pub const EXPORT_FORMAT: &str = "tricky3d-repair-script";
/// The `format` field of scripts exported before the app was renamed from Meshbench to tricky3D.
/// Read, never written.
const OLD_EXPORT_FORMAT: &str = "meshbench-repair-script";
/// The export format version this tricky3D writes.
pub const EXPORT_VERSION: u64 = 1;

/// Reads one of our own exported `.json` scripts: the export format (`export_json`) or a bare
/// `RepairScript` object. Actions tricky3D does not know, or does not offer yet, are skipped with
/// a warning, parameter values that do not fit get the default (with a warning), and every step
/// gets all of its action's parameters. The script gets a fresh id.
pub fn import_json(text: &str) -> Result<Imported, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let value: Value =
        serde_json::from_str(text).map_err(|e| format!("it is not valid JSON ({e})"))?;
    let Value::Object(object) = value else {
        return Err("it is not a repair script".into());
    };
    let mut warnings = Vec::new();
    match object.get("format") {
        None => {}
        Some(Value::String(format)) if format == EXPORT_FORMAT || format == OLD_EXPORT_FORMAT => {
            if object
                .get("version")
                .and_then(Value::as_u64)
                .is_some_and(|v| v > EXPORT_VERSION)
            {
                warnings.push("The script comes from a newer tricky3D; anything this version does not know was left out.".into());
            }
        }
        Some(_) => return Err("it is not a tricky3D repair script".into()),
    }
    let Some(Value::Array(list)) = object.get("actions") else {
        return Err("it has no list of repair actions".into());
    };
    let mut actions = Vec::new();
    for (n, entry) in list.iter().enumerate() {
        let n = n + 1;
        let Some(name) = entry.get("type").and_then(Value::as_str) else {
            warnings.push(format!("Skipped action {n}, which names no repair action."));
            continue;
        };
        let Ok(action) = serde_json::from_value::<ActionId>(Value::String(name.into())) else {
            warnings.push(format!(
                "Skipped action {n} (\"{name}\"), which tricky3D does not know."
            ));
            continue;
        };
        let label = defaults::label_of(action);
        if !defaults::is_implemented(action) {
            warnings.push(format!(
                "Skipped {label} (action {n}), which is not available yet."
            ));
            continue;
        }
        // The window's tools: they need points picked on the part, which no script holds.
        if matches!(action, ActionId::MoveNode | ActionId::CutSurface) {
            warnings.push(format!(
                "Skipped {label} (action {n}), which works on points you pick on the part."
            ));
            continue;
        }
        let given = entry
            .get("params")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let (params, rejected) = complete(action, &given);
        for (spec, value) in rejected {
            warnings.push(format!(
                "Used the default {} for {label} (action {n}): {value} is not valid.",
                spec.label.to_lowercase()
            ));
        }
        actions.push(Step {
            action,
            enabled: entry
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            params,
        });
    }
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .and_then(|n| clean_name(Some(n)))
        .unwrap_or_else(|| IMPORTED_NAME.into());
    Ok(Imported {
        script: Script {
            id: fresh_id(),
            name,
            built_in: false,
            actions,
        },
        warnings,
    })
}

/// Our export format for one script: `{"format": "tricky3d-repair-script", "version": 1,
/// "name": …, "actions": [step, …]}`, pretty-printed.
pub fn export_json(script: &Script) -> String {
    #[derive(Serialize)]
    struct Export<'a> {
        format: &'a str,
        version: u64,
        name: &'a str,
        actions: &'a [Step],
    }
    let export = Export {
        format: EXPORT_FORMAT,
        version: EXPORT_VERSION,
        name: &script.name,
        actions: &script.actions,
    };
    // Serialising strings, booleans and JSON maps cannot fail.
    let mut text = serde_json::to_string_pretty(&export).unwrap_or_default();
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repair::defaults::{DEFAULT_ID, EXTENDED_ID, SIMPLE_ID, builtin_script, catalog};

    fn step(action: ActionId, params: Value) -> Step {
        Step {
            action,
            enabled: true,
            params: params.as_object().cloned().unwrap_or_default(),
        }
    }

    // -----------------------------------------------------------------------------------------
    // parse_action

    #[test]
    fn missing_parameters_take_the_catalog_defaults() {
        assert_eq!(
            parse_action(&step(ActionId::StitchTriangles, json!({}))),
            Ok(Action::StitchTriangles(defaults::STITCH))
        );
        assert_eq!(
            parse_action(&step(ActionId::SanitizeDuplicateNodes, json!({}))),
            Ok(Action::SanitizeDuplicateNodes(SanitizeParams {
                tolerance: 0.0001
            }))
        );
        assert_eq!(
            parse_action(&step(ActionId::RemoveDegenerateFaces, json!({}))),
            Ok(Action::RemoveDegenerateFaces(defaults::DEGENERATE))
        );
        assert_eq!(
            parse_action(&step(ActionId::RemoveTinyShells, json!({}))),
            Ok(Action::RemoveTinyShells(defaults::TINY_SHELLS))
        );
        // Null counts as missing.
        assert_eq!(
            parse_action(&step(
                ActionId::StitchTriangles,
                json!({ "tolerance": null, "ignore_orientation": true })
            )),
            Ok(Action::StitchTriangles(StitchParams {
                tolerance: 0.01,
                ignore_orientation: true
            }))
        );
    }

    #[test]
    fn given_parameters_are_used_and_unknown_ones_ignored() {
        assert_eq!(
            parse_action(&step(
                ActionId::RemoveTinyShells,
                json!({ "mode": "area", "min_volume": 2, "min_area": 0.5, "colour": "red" })
            )),
            Ok(Action::RemoveTinyShells(TinyShellParams {
                mode: TinyShellMode::Area,
                min_volume: 2.0,
                min_area: 0.5,
            }))
        );
        assert_eq!(
            parse_action(&step(
                ActionId::RemoveDegenerateFaces,
                json!({ "calc_tolerance": false, "tolerance": 0 })
            )),
            Ok(Action::RemoveDegenerateFaces(DegenerateParams {
                calc_tolerance: false,
                tolerance: 0.0,
            }))
        );
        // Parameters on an action that has none are ignored too.
        assert_eq!(
            parse_action(&step(ActionId::WrapPartSurface, json!({ "tolerance": -5 }))),
            Ok(Action::WrapPartSurface)
        );
    }

    #[test]
    fn bad_values_fail_with_a_reason_naming_the_action_and_parameter() {
        let fails = |action, params: Value| parse_action(&step(action, params)).unwrap_err();
        let tolerance = "Stitch triangles: the tolerance must be a number of 0 mm or more";
        assert_eq!(
            fails(ActionId::StitchTriangles, json!({ "tolerance": -0.1 })),
            tolerance
        );
        assert_eq!(
            fails(ActionId::StitchTriangles, json!({ "tolerance": "0.01" })),
            tolerance
        );
        assert_eq!(
            fails(ActionId::StitchTriangles, json!({ "tolerance": true })),
            tolerance
        );
        assert_eq!(
            fails(
                ActionId::StitchTriangles,
                json!({ "ignore_orientation": 1 })
            ),
            "Stitch triangles: \"Ignore orientation\" must be yes or no"
        );
        assert_eq!(
            fails(
                ActionId::RemoveDegenerateFaces,
                json!({ "calc_tolerance": "yes" })
            ),
            "Remove degenerate faces: \"Calculate tolerance by bounding box\" must be yes or no"
        );
        assert_eq!(
            fails(ActionId::RemoveTinyShells, json!({ "min_volume": -1 })),
            "Remove tiny shells: the minimum volume must be a number of 0 mm³ or more"
        );
        assert_eq!(
            fails(ActionId::RemoveTinyShells, json!({ "min_area": [] })),
            "Remove tiny shells: the minimum area must be a number of 0 mm² or more"
        );
        for mode in [json!("everything"), json!(""), json!(0)] {
            assert_eq!(
                fails(ActionId::RemoveTinyShells, json!({ "mode": mode })),
                "Remove tiny shells: the tiny-shell detection must be one of the choices tricky3D offers"
            );
        }
        assert_eq!(
            fails(
                ActionId::SanitizeDuplicateNodes,
                json!({ "tolerance": -1e-9 })
            ),
            "Sanitize duplicate nodes: the tolerance must be a number of 0 mm or more"
        );
        // Zero is allowed, and -0 is zero.
        assert!(
            parse_action(&step(
                ActionId::SanitizeDuplicateNodes,
                json!({ "tolerance": -0.0 })
            ))
            .is_ok()
        );
    }

    #[test]
    fn soon_actions_are_not_available_yet() {
        for (action, label) in [
            (ActionId::Remesh, "Remesh"),
            (ActionId::PrefixSuffix, "Prefix-Suffix"),
            (ActionId::ZCompensation, "Z-compensation"),
        ] {
            assert_eq!(
                parse_action(&step(action, json!({}))),
                Err(format!("{label} is not available yet"))
            );
        }
    }

    #[test]
    fn the_interactive_tools_read_what_the_window_sends() {
        assert_eq!(
            parse_action(&step(
                ActionId::MoveNode,
                json!({ "vertex": 7, "x": 1.5, "y": -2.0, "z": 0.25 })
            )),
            Ok(Action::MoveNode(NodeMove {
                vertex: 7,
                to: [1.5, -2.0, 0.25]
            }))
        );
        assert_eq!(
            parse_action(&step(ActionId::MoveNode, json!({ "vertex": 7 }))),
            Err("Move nodes: the point to move or its new position is missing".into())
        );
        let line = json!({
            "points": [[0.0, 1.0, 2.0, 4], [3.0, 4.0, 5.0, 9]],
            "smooth": true,
            "snap": 0.25,
            "closed": true,
        });
        assert_eq!(
            parse_action(&step(ActionId::CutSurface, line)),
            Ok(Action::CutSurface(CutLine {
                points: vec![
                    CutPoint {
                        at: [0.0, 1.0, 2.0],
                        triangle: 4
                    },
                    CutPoint {
                        at: [3.0, 4.0, 5.0],
                        triangle: 9
                    },
                ],
                options: CutOptions {
                    smooth: true,
                    step: 2.0,
                    snap: 0.25,
                    closed: true
                },
            }))
        );
        for bad in [
            json!({}),
            json!({ "points": [[0.0, 1.0, 2.0]] }),
            json!({ "points": [[0.0, 1.0, 2.0, -1]] }),
        ] {
            assert_eq!(
                parse_action(&step(ActionId::CutSurface, bad)),
                Err("Cut surfaces: the points of the line are damaged".into())
            );
        }
        assert_eq!(
            parse_action(&step(
                ActionId::CutSurface,
                json!({ "points": [], "step": 0.001 })
            )),
            Err("Cut surfaces: the smooth step length must be a number of 0.01 mm or more".into())
        );
        assert_eq!(
            parse_action(&step(
                ActionId::CutSurface,
                json!({ "points": [], "snap": -1 })
            )),
            Err("Cut surfaces: the snap distance must be a number of 0 mm or more".into())
        );
    }

    #[test]
    fn every_catalog_action_parses_exactly_when_it_is_implemented() {
        for spec in catalog().actions {
            // The two interactive tools take what the window sends; nothing else holds it.
            let params = match spec.id {
                ActionId::MoveNode => json!({ "vertex": 3, "x": 1.0, "y": 2.0, "z": 3.0 }),
                ActionId::CutSurface => json!({ "points": [[0.0, 0.0, 0.0, 0]] }),
                _ => json!({}),
            };
            let parsed = parse_action(&step(spec.id, params));
            assert_eq!(parsed.is_ok(), spec.implemented, "{}", spec.label);
            if let Ok(action) = parsed {
                assert_eq!(action.id(), spec.id);
            }
            // Each catalog default is valid for its own parameter.
            for p in &spec.params {
                assert!(valid(p, &p.default), "{} {}", spec.label, p.id);
            }
        }
    }

    #[test]
    fn every_builtin_step_parses_to_the_catalog_defaults_or_its_own_values() {
        for id in [SIMPLE_ID, DEFAULT_ID, EXTENDED_ID] {
            for s in builtin_script(id).unwrap().actions {
                parse_action(&s).unwrap();
            }
        }
        let default = builtin_script(DEFAULT_ID).unwrap().actions;
        assert_eq!(
            parse_action(&default[5]),
            Ok(Action::StitchTriangles(StitchParams {
                tolerance: 0.01,
                ignore_orientation: true
            }))
        );
    }

    #[test]
    fn steps_read_from_window_json() {
        // How the window sends a step: unknown params kept in the map, enabled optional.
        let s: Step = serde_json::from_value(json!({
            "type": "stitch_triangles",
            "params": { "tolerance": 0.02, "extra": 1 }
        }))
        .unwrap();
        assert!(s.enabled);
        assert_eq!(
            parse_action(&s),
            Ok(Action::StitchTriangles(StitchParams {
                tolerance: 0.02,
                ignore_orientation: false
            }))
        );
        assert!(serde_json::from_value::<Step>(json!({ "type": "explode" })).is_err());
    }

    // -----------------------------------------------------------------------------------------
    // .repair files

    /// A `.repair` list around the given `<repair>` elements, with CRLF line ends.
    fn repair_list(name: &str, repairs: &[&str]) -> String {
        let mut text = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<repairlist>\r\n\t<name>{name}</name>\r\n"
        );
        for r in repairs {
            text.push_str(r);
            text.push_str("\r\n");
        }
        text.push_str("</repairlist>\r\n");
        text
    }

    fn option(id: &str, kind: &str, value: &str) -> String {
        format!("<option><id>{id}</id><type>{kind}</type><value>{value}</value></option>")
    }

    #[test]
    fn repair_types_and_options_map_to_actions_and_parameters() {
        let stitch = format!(
            "<repair enabled=\"1\">\r\n<type>1</type>{}{}</repair>",
            option("1", "float", "0.05"),
            option("2", "boolean", "1")
        );
        let tiny = format!(
            "<repair enabled=\"0\"><type> 7 </type>{}{}{}</repair>",
            option("mode", "string", ""),
            option("4", "float", "2.5"),
            option("minarea", "float", "0.25")
        );
        let text = repair_list(
            "My list",
            &[
                &stitch,
                "<repair enabled=\"1\"><type>13</type></repair>",
                &tiny,
            ],
        );
        let imported = import_repair_xml(&text).unwrap();
        assert!(imported.warnings.is_empty(), "{:?}", imported.warnings);
        let script = imported.script;
        assert_eq!(script.name, "My list");
        assert!(script.id.starts_with("own."));
        assert!(!script.built_in);
        assert_eq!(script.actions.len(), 3);
        assert_eq!(
            script.actions[0],
            step(
                ActionId::StitchTriangles,
                json!({ "tolerance": 0.05, "ignore_orientation": true })
            )
        );
        assert_eq!(
            script.actions[1],
            step(ActionId::WrapPartSurface, json!({}))
        );
        assert!(!script.actions[2].enabled);
        assert_eq!(
            script.actions[2].params,
            *json!({ "mode": "closed_volume_open_area", "min_volume": 2.5, "min_area": 0.25 })
                .as_object()
                .unwrap()
        );
    }

    #[test]
    fn unknown_types_and_options_are_skipped_with_warnings() {
        let text = repair_list(
            "Odd",
            &[
                "<repair enabled=\"1\"><type>9</type></repair>",
                &format!(
                    "<repair><type>1</type>{}{}</repair>",
                    option("5", "float", "1"),
                    option("1", "float", "0.2")
                ),
                &format!(
                    "<repair><type>6</type>{}</repair>",
                    option("3", "float", "-1")
                ),
                "<repair><type>x</type></repair>",
                "<repair enabled=\"1\"/>",
                &format!(
                    "<repair><type>2</type>{}</repair>",
                    option("1", "float", "3")
                ),
                &format!(
                    "<repair><type>6</type>{}</repair>",
                    option("calc_tolerance", "boolean", "maybe")
                ),
                &format!(
                    "<repair><type>7</type>{}</repair>",
                    option("mode", "string", "17")
                ),
            ],
        );
        let imported = import_repair_xml(&text).unwrap();
        assert_eq!(
            imported.warnings,
            [
                "Skipped action type 9, which tricky3D does not know.",
                "Skipped option 5 of Stitch triangles (action 2), which tricky3D does not know.",
                "Used the default tolerance for Remove degenerate faces (action 3): \"-1\" is not valid.",
                "Skipped action 4, whose type \"x\" is not a number.",
                "Skipped action 5, which has no type number.",
                "Skipped option 1 of Fix flipped triangles (action 6), which tricky3D does not know.",
                "Used the default calculate tolerance by bounding box for Remove degenerate faces (action 7): \"maybe\" is not valid.",
                "Used the default tiny-shell detection for Remove tiny shells (action 8): \"17\" is not valid.",
            ]
        );
        let actions: Vec<ActionId> = imported.script.actions.iter().map(|s| s.action).collect();
        assert_eq!(
            actions,
            [
                ActionId::StitchTriangles,
                ActionId::RemoveDegenerateFaces,
                ActionId::FixFlippedTriangles,
                ActionId::RemoveDegenerateFaces,
                ActionId::RemoveTinyShells,
            ]
        );
        // A <repair> without `enabled` is on; the kept option still counts.
        assert!(imported.script.actions[0].enabled);
        assert_eq!(imported.script.actions[0].params["tolerance"], json!(0.2));
        assert_eq!(imported.script.actions[1].params["tolerance"], json!(0.002));
    }

    #[test]
    fn whitespace_case_entities_and_unknown_elements_are_tolerated() {
        let text = "\u{feff}<?xml version=\"1.0\"?>\n<!-- exported -->\n<RepairList>\n  <Name>  A &amp; B  </Name>\n  <version>2</version>\n  <repair enabled=\" TRUE \">\n    <type>\n      1\n    </type>\n    <description><b>nested</b></description>\n    <option>\n      <id> 01 </id>\n      <value>\n 0.03 \n</value>\n    </option>\n    <option><id>2</id><value>False</value></option>\n  </repair>\n  <repair enabled=\"false\"><type>3</type></repair>\n</RepairList>\n";
        let imported = import_repair_xml(text).unwrap();
        assert!(imported.warnings.is_empty(), "{:?}", imported.warnings);
        assert_eq!(imported.script.name, "A & B");
        assert_eq!(
            imported.script.actions,
            vec![
                step(
                    ActionId::StitchTriangles,
                    json!({ "tolerance": 0.03, "ignore_orientation": false })
                ),
                Step {
                    action: ActionId::CloseTrivialHoles,
                    enabled: false,
                    params: Map::new(),
                },
            ]
        );
    }

    #[test]
    fn a_decimal_comma_reads_as_a_point() {
        let stitch = |value: &str| {
            format!(
                "<repair><type>1</type>{}</repair>",
                option("1", "float", value)
            )
        };
        let text = repair_list(
            "Comma",
            &[&stitch("0,02"), &stitch("1,000,5"), &stitch("1.000,5")],
        );
        let imported = import_repair_xml(&text).unwrap();
        let tolerances: Vec<&Value> = imported
            .script
            .actions
            .iter()
            .map(|s| &s.params["tolerance"])
            .collect();
        assert_eq!(tolerances, [&json!(0.02), &json!(0.01), &json!(0.01)]);
        assert_eq!(
            imported.warnings,
            [
                "Used the default tolerance for Stitch triangles (action 2): \"1,000,5\" is not valid.",
                "Used the default tolerance for Stitch triangles (action 3): \"1.000,5\" is not valid.",
            ]
        );
    }

    #[test]
    fn a_file_that_is_not_a_repair_list_is_refused() {
        assert_eq!(
            import_repair_xml("<amf><object/></amf>").unwrap_err(),
            "it is not a repair list (it has no <repairlist> element)"
        );
        assert!(import_repair_xml("").is_err());
        assert!(
            import_repair_xml("<repairlist><repair></repairlist>")
                .unwrap_err()
                .starts_with("its repair list is not valid XML")
        );
        // A list with nothing usable still imports, empty and with warnings.
        let imported =
            import_repair_xml("<repairlist><repair><type>99</type></repair></repairlist>").unwrap();
        assert!(imported.script.actions.is_empty());
        assert_eq!(imported.script.name, "Imported script");
        assert_eq!(imported.warnings.len(), 1);
    }

    #[test]
    fn imported_scripts_get_distinct_fresh_ids() {
        let a = import_repair_xml("<repairlist/>").unwrap().script.id;
        let b = import_repair_xml("<repairlist/>").unwrap().script.id;
        let c = import_json("{\"actions\": []}").unwrap().script.id;
        assert!(a.starts_with("own.") && b.starts_with("own.") && c.starts_with("own."));
        assert!(a != b && b != c && a != c);
        let number = |id: &str| id["own.".len()..].parse::<u64>().unwrap();
        assert!(number(&b) > number(&a) && number(&c) > number(&b));
    }

    // -----------------------------------------------------------------------------------------
    // Our JSON

    #[test]
    fn export_and_import_round_trip() {
        for id in [SIMPLE_ID, DEFAULT_ID, EXTENDED_ID] {
            let builtin = builtin_script(id).unwrap();
            let text = export_json(&builtin);
            let value: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["format"], "tricky3d-repair-script");
            assert_eq!(value["version"], 1);
            assert_eq!(value["name"], builtin.name.as_str());
            assert!(value.get("id").is_none() && value.get("builtIn").is_none());
            let back = import_json(&text).unwrap();
            assert!(back.warnings.is_empty(), "{:?}", back.warnings);
            assert_eq!(back.script.name, builtin.name);
            assert_eq!(back.script.actions, builtin.actions);
            assert!(!back.script.built_in);
            assert_ne!(back.script.id, builtin.id);
        }
        // An own script with a disabled step and values of its own.
        let own = Script {
            id: "own.5".into(),
            name: "Mine: \"quoted\" ü".into(),
            built_in: false,
            actions: vec![
                Step {
                    action: ActionId::SanitizeDuplicateNodes,
                    enabled: false,
                    params: json!({ "tolerance": 0.5 }).as_object().cloned().unwrap(),
                },
                step(
                    ActionId::StitchTriangles,
                    json!({ "tolerance": 1e-7, "ignore_orientation": true }),
                ),
            ],
        };
        let back = import_json(&export_json(&own)).unwrap();
        assert_eq!(back.script.name, own.name);
        assert_eq!(back.script.actions, own.actions);
        assert!(export_json(&own).ends_with("}\n"));
    }

    #[test]
    fn json_import_takes_a_bare_script_and_completes_its_parameters() {
        let text = r#"{ "id": "builtin.extended", "name": "  Bare  ", "builtIn": true, "actions": [
            { "type": "stitch_triangles", "params": { "tolerance": 0.2 } },
            { "type": "close_all_holes", "enabled": false },
            { "type": "remove_tiny_shells", "params": { "mode": "volume", "min_area": "big", "x": 1 } }
        ] }"#;
        let imported = import_json(text).unwrap();
        assert_eq!(
            imported.warnings,
            [
                "Used the default minimum area for Remove tiny shells (action 3): \"big\" is not valid."
            ]
        );
        let script = imported.script;
        assert_eq!(script.name, "Bare");
        assert!(!script.built_in);
        assert!(script.id.starts_with("own."));
        assert_eq!(
            script.actions,
            vec![
                step(
                    ActionId::StitchTriangles,
                    json!({ "tolerance": 0.2, "ignore_orientation": false })
                ),
                Step {
                    action: ActionId::CloseAllHoles,
                    enabled: false,
                    params: Map::new()
                },
                step(
                    ActionId::RemoveTinyShells,
                    json!({ "mode": "volume", "min_volume": 0.0001, "min_area": 0.0001 })
                ),
            ]
        );
    }

    #[test]
    fn json_import_skips_what_it_cannot_run() {
        let text = r#"{ "format": "tricky3d-repair-script", "version": 7, "name": "Future", "actions": [
            { "type": "remesh" },
            { "type": "teleport" },
            { "params": {} },
            "close_all_holes",
            { "type": "fix_flipped_triangles", "enabled": "no" },
            { "type": "cut_surface", "params": { "points": [] } },
            { "type": "move_node" }
        ] }"#;
        let imported = import_json(text).unwrap();
        assert_eq!(
            imported.warnings,
            [
                "The script comes from a newer tricky3D; anything this version does not know was left out.",
                "Skipped Remesh (action 1), which is not available yet.",
                "Skipped action 2 (\"teleport\"), which tricky3D does not know.",
                "Skipped action 3, which names no repair action.",
                "Skipped action 4, which names no repair action.",
                "Skipped Cut surfaces (action 6), which works on points you pick on the part.",
                "Skipped Move nodes (action 7), which works on points you pick on the part.",
            ]
        );
        assert_eq!(
            imported.script.actions,
            vec![step(ActionId::FixFlippedTriangles, json!({}))]
        );
    }

    #[test]
    fn json_exported_before_the_rename_imports() {
        let text = r#"{ "format": "meshbench-repair-script", "version": 1, "name": "Old", "actions": [
            { "type": "fix_flipped_triangles" }
        ] }"#;
        let imported = import_json(text).unwrap();
        assert_eq!(imported.warnings, Vec::<String>::new());
        assert_eq!(imported.script.name, "Old");
        assert_eq!(
            imported.script.actions,
            vec![step(ActionId::FixFlippedTriangles, json!({}))]
        );
    }

    #[test]
    fn json_that_is_not_a_script_is_refused() {
        assert!(
            import_json("not json")
                .unwrap_err()
                .starts_with("it is not valid JSON")
        );
        assert_eq!(
            import_json("[1, 2]").unwrap_err(),
            "it is not a repair script"
        );
        assert_eq!(
            import_json(r#"{ "format": "other", "actions": [] }"#).unwrap_err(),
            "it is not a tricky3D repair script"
        );
        assert_eq!(
            import_json(r#"{ "name": "x" }"#).unwrap_err(),
            "it has no list of repair actions"
        );
        // A byte order mark and CRLF line ends are fine.
        let text =
            "\u{feff}{\r\n\"actions\": [\r\n{\"type\": \"close_trivial_holes\"}\r\n]\r\n}\r\n";
        let imported = import_json(text).unwrap();
        assert_eq!(imported.script.name, "Imported script");
        assert_eq!(imported.script.actions.len(), 1);
    }

    #[test]
    fn stored_scripts_read_what_the_window_writes() {
        let stored: StoredScripts = serde_json::from_value(json!({
            "scripts": [{ "id": "own.1", "name": "A", "builtIn": false, "actions": [
                { "type": "close_all_holes", "enabled": true, "params": {} }
            ] }],
            "order": ["builtin.simple", "own.1"]
        }))
        .unwrap();
        assert_eq!(stored.scripts[0].actions[0].action, ActionId::CloseAllHoles);
        let back = serde_json::to_value(&stored).unwrap();
        assert_eq!(back["scripts"][0]["builtIn"], false);
        assert_eq!(back["scripts"][0]["actions"][0]["type"], "close_all_holes");
        assert_eq!(back["order"][1], "own.1");
        // An older file without an order still reads.
        let bare: StoredScripts = serde_json::from_value(json!({ "scripts": [] })).unwrap();
        assert!(bare.order.is_empty());
    }
}
