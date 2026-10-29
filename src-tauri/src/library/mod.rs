//! The part library: closed, outward-facing meshes made to the values the user sets, in four
//! categories (Primitive, Design, Mathematical, Quality), and the catalog of their parameters,
//! which the window builds its form from (src/library). A made part sits centred on the local Z
//! axis with its lowest point at z = 0, so it rests on the platform where it is placed; parts made
//! from the selected parts (Convex hull, Base plate) come back in world space instead.
//!
//! Every generator builds a mesh that is closed, oriented and free of self-intersections, so
//! Repair's status shows it clean (the tests check each one).

mod build;
mod design;
mod expr;
mod gear;
mod hull;
mod math;
mod primitive;
mod qr;
mod quality;
mod samples;
mod solid;
mod terrace;
mod threads;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::mesh::{Mesh, fmt_count};
use crate::repair::defaults::{DEGENERATE, effective_degenerate_tolerance};
use crate::repair::{Control, RepairError, RepairResult, degenerate, status};

pub(crate) use build::Polygon;
pub use qr::{Ecc, encode as encode_qr};
pub use threads::{METRIC, Metric};

/// More triangles than this would take long to draw and help nobody.
pub const MAX_TRIANGLES: usize = 4_000_000;

pub(crate) const MAX_MM: f64 = 10_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Primitive,
    Design,
    Mathematical,
    Quality,
}

/// One option of a choice. `soon` options show but cannot be picked yet.
#[derive(Clone, Debug, Serialize)]
pub struct Choice {
    pub value: &'static str,
    pub label: &'static str,
    pub soon: bool,
}

/// What a parameter holds. Field names match `LibraryParam` in src/library/ipc.ts.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Kind {
    /// A number: "mm", "°", "%" or "" (a count or a plain number).
    #[serde(rename_all = "camelCase")]
    Number {
        unit: &'static str,
        default: f64,
        min: f64,
        max: f64,
        /// Whole numbers only (counts).
        integer: bool,
    },
    Choice {
        default: &'static str,
        options: Vec<Choice>,
    },
    #[serde(rename_all = "camelCase")]
    Text {
        default: &'static str,
        max_length: usize,
    },
    Toggle {
        default: bool,
    },
    /// A picture the window decodes to grey levels; without one, a built-in sample picture.
    Image,
}

/// A parameter of a library item. Field names match `LibraryParam` in src/library/ipc.ts.
#[derive(Clone, Debug, Serialize)]
pub struct Param {
    pub key: &'static str,
    pub label: &'static str,
    /// One sentence for the field's tooltip; may be empty.
    pub help: &'static str,
    #[serde(flatten)]
    pub kind: Kind,
    /// Shown only while the toggle with this key is on.
    pub when: Option<&'static str>,
}

/// What an item is made from besides its parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Input {
    None,
    /// The selected parts, in world space; the result comes back in world space too.
    Parts,
}

/// An item of the library. Field names match `LibraryItem` in src/library/ipc.ts.
#[derive(Clone, Debug, Serialize)]
pub struct Item {
    pub id: &'static str,
    pub label: &'static str,
    pub category: Category,
    /// One sentence for the tile's tooltip.
    pub description: &'static str,
    /// Why the item is not ready yet (shown as Soon); None when it is.
    pub soon: Option<&'static str>,
    pub input: Input,
    pub params: Vec<Param>,
}

// ---------------------------------------------------------------------------------------------
// Parameter shorthands

pub(crate) const fn mm(key: &'static str, label: &'static str, default: f64) -> Param {
    num(key, label, "mm", default, 0.001, MAX_MM)
}

pub(crate) const fn num(
    key: &'static str,
    label: &'static str,
    unit: &'static str,
    default: f64,
    min: f64,
    max: f64,
) -> Param {
    Param {
        key,
        label,
        help: "",
        kind: Kind::Number {
            unit,
            default,
            min,
            max,
            integer: false,
        },
        when: None,
    }
}

pub(crate) const fn count(
    key: &'static str,
    label: &'static str,
    default: f64,
    min: f64,
    max: f64,
) -> Param {
    Param {
        key,
        label,
        help: "",
        kind: Kind::Number {
            unit: "",
            default,
            min,
            max,
            integer: true,
        },
        when: None,
    }
}

pub(crate) fn choice(
    key: &'static str,
    label: &'static str,
    default: &'static str,
    options: &[(&'static str, &'static str)],
) -> Param {
    Param {
        key,
        label,
        help: "",
        kind: Kind::Choice {
            default,
            options: options
                .iter()
                .map(|&(value, label)| Choice {
                    value,
                    label,
                    soon: false,
                })
                .collect(),
        },
        when: None,
    }
}

pub(crate) const fn toggle(key: &'static str, label: &'static str, default: bool) -> Param {
    Param {
        key,
        label,
        help: "",
        kind: Kind::Toggle { default },
        when: None,
    }
}

pub(crate) const fn text(
    key: &'static str,
    label: &'static str,
    default: &'static str,
    max_length: usize,
) -> Param {
    Param {
        key,
        label,
        help: "",
        kind: Kind::Text {
            default,
            max_length,
        },
        when: None,
    }
}

pub(crate) const fn image(key: &'static str, label: &'static str) -> Param {
    Param {
        key,
        label,
        help: "",
        kind: Kind::Image,
        when: None,
    }
}

impl Param {
    pub(crate) fn help(self, help: &'static str) -> Param {
        Param { help, ..self }
    }

    pub(crate) fn when(self, toggle: &'static str) -> Param {
        Param {
            when: Some(toggle),
            ..self
        }
    }
}

pub(crate) fn item(
    id: &'static str,
    label: &'static str,
    category: Category,
    description: &'static str,
    params: Vec<Param>,
) -> Item {
    Item {
        id,
        label,
        category,
        description,
        soon: None,
        input: Input::None,
        params,
    }
}

impl Item {
    pub(crate) fn soon(self, why: &'static str) -> Item {
        Item {
            soon: Some(why),
            ..self
        }
    }

    pub(crate) fn made_from_parts(self) -> Item {
        Item {
            input: Input::Parts,
            ..self
        }
    }
}

/// The Design tiles in the order the reference lays them out; our own sample models take the places
/// of its sample models.
const DESIGN_ORDER: [&str; 28] = [
    "rect_grid",
    "hex_grid",
    "hex_cylinder",
    "square_grid",
    "bitmap",
    "heightmap",
    "qr_code",
    "bottle_opener",
    "text",
    "screw",
    "nut",
    "gear",
    "whistle",
    "mold",
    "vase",
    "picture_sweep",
    "bracelet",
    "brick",
    "mesh_on_spline",
    "toy_car",
    "small_house",
    "toy_plane",
    "boat",
    "rocket",
    "chess_pawn",
    "robot",
    "light_gear",
    "base_plate",
];

/// Every item with its parameters, in the order the library shows them: each category's in turn.
pub fn catalog() -> Vec<Item> {
    let mut all = primitive::catalog();
    let mut design = design::catalog();
    design.extend(samples::catalog());
    design.sort_by_key(|i| DESIGN_ORDER.iter().position(|&id| id == i.id));
    all.extend(design);
    all.extend(math::catalog());
    all.extend(quality::catalog());
    all
}

// ---------------------------------------------------------------------------------------------
// Values

/// The values of a request, each checked against the item's parameters; a missing one takes its
/// default. Errors are sentences for the user.
pub(crate) struct Values<'a> {
    spec: &'a Item,
    given: &'a Map<String, Value>,
    /// Building a tile picture: field-based items may use a coarser grid.
    pub(crate) preview: bool,
}

impl Values<'_> {
    fn param(&self, key: &str) -> &Param {
        self.spec
            .params
            .iter()
            .find(|p| p.key == key)
            .unwrap_or_else(|| panic!("{} has no parameter {key}", self.spec.id))
    }

    pub(crate) fn get(&self, key: &str) -> Result<f64, String> {
        let p = self.param(key);
        let Kind::Number {
            unit,
            default,
            min,
            max,
            integer,
        } = p.kind
        else {
            panic!("{key} is not a number");
        };
        let v = match self.given.get(key) {
            None | Some(Value::Null) => default,
            Some(v) => v
                .as_f64()
                .ok_or_else(|| format!("{} must be a number.", p.label))?,
        };
        if !v.is_finite() || v < min || v > max || (integer && v.fract() != 0.0) {
            let unit = if unit.is_empty() {
                String::new()
            } else if unit == "%" || unit == "°" {
                unit.to_string()
            } else {
                format!(" {unit}")
            };
            let kind = if integer { "a whole number " } else { "" };
            return Err(format!(
                "{} must be {kind}from {min}{unit} to {max}{unit}.",
                p.label
            ));
        }
        Ok(v)
    }

    pub(crate) fn count(&self, key: &str) -> Result<usize, String> {
        self.get(key).map(|v| v as usize)
    }

    pub(crate) fn choice(&self, key: &str) -> Result<&'static str, String> {
        let p = self.param(key);
        let Kind::Choice { default, options } = &p.kind else {
            panic!("{key} is not a choice");
        };
        let picked = match self.given.get(key) {
            None | Some(Value::Null) => *default,
            Some(v) => v
                .as_str()
                .ok_or_else(|| format!("{} must be one of its choices.", p.label))?,
        };
        match options.iter().find(|o| o.value == picked) {
            Some(o) if o.soon => Err(format!("{} is not available yet.", o.label)),
            Some(o) => Ok(o.value),
            None => Err(format!("{} must be one of its choices.", p.label)),
        }
    }

    pub(crate) fn toggle(&self, key: &str) -> Result<bool, String> {
        let p = self.param(key);
        let Kind::Toggle { default } = p.kind else {
            panic!("{key} is not a toggle");
        };
        match self.given.get(key) {
            None | Some(Value::Null) => Ok(default),
            Some(v) => v
                .as_bool()
                .ok_or_else(|| format!("{} must be on or off.", p.label)),
        }
    }

    pub(crate) fn text(&self, key: &str) -> Result<String, String> {
        let p = self.param(key);
        let Kind::Text {
            default,
            max_length,
        } = p.kind
        else {
            panic!("{key} is not a text");
        };
        let text = match self.given.get(key) {
            None | Some(Value::Null) => default.to_string(),
            Some(v) => v
                .as_str()
                .ok_or_else(|| format!("{} must be text.", p.label))?
                .to_string(),
        };
        if text.chars().count() > max_length {
            return Err(format!(
                "{} can hold at most {max_length} characters.",
                p.label
            ));
        }
        Ok(text)
    }
}

// ---------------------------------------------------------------------------------------------
// Making parts

/// A picture as grey levels: `gray[row * width + column]` from 0 (black) to 1 (white), row 0 at
/// the top, as the window decoded it.
#[derive(Clone, Debug)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub gray: Vec<f32>,
}

/// What an item is made from besides its parameters.
#[derive(Default)]
pub struct Sources {
    /// The selected parts in world space (Input::Parts).
    pub parts: Vec<Mesh>,
    /// The picture picked for an Image parameter; None uses the built-in sample.
    pub picture: Option<Picture>,
}

/// A made part.
#[derive(Debug)]
pub struct Made {
    pub mesh: Mesh,
    /// True when the mesh is in world space (made from the selected parts), false when it stands
    /// centred on the local Z axis at z = 0.
    pub world: bool,
}

/// The mesh of item `id` with `params` (key → value). Errors with a sentence saying what is
/// wrong, or Cancelled. `preview` builds the tile picture: field-based items use a coarser grid.
pub fn make(
    id: &str,
    params: &Map<String, Value>,
    sources: &Sources,
    preview: bool,
    control: &mut Control,
) -> RepairResult<Made> {
    let all = catalog();
    let spec = all
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| RepairError::Failed(format!("The part library has no item \"{id}\".")))?;
    if let Some(why) = spec.soon {
        return Err(RepairError::Failed(format!(
            "{} is not available yet: {why}",
            spec.label
        )));
    }
    let v = Values {
        spec,
        given: params,
        preview,
    };
    if spec.input == Input::Parts && sources.parts.is_empty() && !preview {
        return Err(RepairError::Failed(format!(
            "Select the part to make the {} from first.",
            spec.label.to_lowercase()
        )));
    }
    let failed = RepairError::Failed;
    let mesh = match spec.category {
        Category::Primitive => primitive::make(id, &v).map_err(failed)?,
        Category::Design if samples::has(id) => samples::make(id, &v, control)?,
        Category::Design => design::make(id, &v, sources, control)?,
        Category::Mathematical => math::make(id, &v, sources, control)?,
        Category::Quality => quality::make(id, &v, control)?,
    };
    if mesh.triangle_count() > MAX_TRIANGLES {
        return Err(RepairError::Failed(format!(
            "That would make {} triangles; lower the resolution to stay under {}.",
            fmt_count(mesh.triangle_count() as u64),
            fmt_count(MAX_TRIANGLES as u64)
        )));
    }
    if mesh.triangle_count() == 0 {
        return Err(RepairError::Failed(
            "Those values leave nothing to make.".into(),
        ));
    }
    // Wrap part surface leaves edges one 0.002 mm grid step long; moving the part to the origin can
    // round a few of them a hair shorter, below the tolerance Repair's status counts degenerate
    // triangles at. Those are collapsed, as Remove degenerate faces does.
    let tolerance = effective_degenerate_tolerance(&DEGENERATE);
    let mesh = if status::degenerate_triangles(&mesh, tolerance).is_empty() {
        mesh
    } else {
        degenerate::run(&mesh, &[], &DEGENERATE, control)?.mesh
    };
    // Parts made from the selection stay where it is, unless the tile picture used the sample.
    let world = spec.input == Input::Parts && !sources.parts.is_empty();
    Ok(Made { mesh, world })
}

/// A cylinder of `radius` and `height` standing centred on the Z axis on z = 0.
pub(crate) fn primitive_cylinder(
    radius: f64,
    height: f64,
    segments: usize,
) -> Result<Mesh, String> {
    primitive::frustum_of_cone(radius, Some(radius), height, segments)
}

/// `make` without sources or cancelling, for tests and callers that only have numbers.
pub fn generate(id: &str, params: &Map<String, Value>) -> Result<Mesh, String> {
    crate::repair::silent(|c| make(id, params, &Sources::default(), false, c))
        .map(|m| m.mesh)
        .map_err(|e| match e {
            RepairError::Failed(why) => why,
            RepairError::Cancelled => "Cancelled.".into(),
        })
}

#[cfg(test)]
pub(crate) mod tests;
