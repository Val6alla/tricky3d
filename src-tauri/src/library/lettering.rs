//! Library items made with the label code (src/label), in the bundled font: Text (letters as a
//! solid), Mold (a plate with the text raised or sunk, mirrored for casting) and Whistle (a
//! whistle of our own design with its caption on the side).

use super::build::{Builder, extrude};
use super::solid::field;
use super::{Category, Item, Values, choice, item, mm, num, text, toggle};
use crate::label::{self, Frame, font::BUNDLED};
use crate::mesh::{Mesh, make_box};
use crate::repair::{Control, RepairError, RepairResult};

/// The font size the library's letters are cut at (see `label::font::text_contours`).
const FONT_SIZE: f64 = 48.0;

/// Whether `id` is one of these items.
pub(super) fn has(id: &str) -> bool {
    matches!(id, "text" | "mold" | "whistle")
}

pub(super) fn catalog() -> Vec<Item> {
    vec![
        item(
            "text",
            "Text",
            Category::Design,
            "Letters as a solid, standing on the platform.",
            vec![
                text("text", "Text", "tricky3D", 200),
                mm("height", "Letter height", 10.0)
                    .help("From the lowest to the highest point of the letters."),
                mm("thickness", "Thickness", 3.0),
            ],
        ),
        item(
            "whistle",
            "Whistle",
            Category::Design,
            "A whistle to print in one piece, with a caption on its side.",
            vec![
                text("text", "Caption", "tricky3D", 40),
                num("scale", "Size", "%", 100.0, 50.0, 300.0)
                    .help("100 % is a whistle 41 mm long."),
            ],
        ),
        item(
            "mold",
            "Mold",
            Category::Design,
            "A plate with text raised or sunk, mirrored so a cast reads the right way round.",
            vec![
                text("text", "Text", "tricky3D", 200),
                mm("height", "Letter height", 10.0),
                choice(
                    "style",
                    "Letters",
                    "sunk",
                    &[
                        ("sunk", "Sunk into the plate"),
                        ("raised", "Raised from the plate"),
                    ],
                ),
                mm("depth", "Depth", 2.0).help("How far the letters are sunk or raised."),
                mm("thickness", "Plate thickness", 4.0),
                mm("margin", "Margin", 5.0).help("The plate's border round the letters."),
                toggle("mirror", "Mirrored", true)
                    .help("Mirror the letters, so what is cast in the mould reads right."),
            ],
        ),
    ]
}

/// The caption at `height` mm in the bundled font, mirrored or not, and its width.
fn caption(text: &str, height: f64, mirrored: bool) -> Result<(label::Region, f64), String> {
    let shape = label::text_shape(BUNDLED, 0, text, FONT_SIZE)?;
    let width = height * shape.aspect;
    Ok((label::sized(&shape, height, width, mirrored), width))
}

pub(super) fn make(id: &str, v: &Values, control: &mut Control) -> RepairResult<Mesh> {
    let failed = RepairError::Failed;
    match id {
        "text" => {
            let (region, _) = caption(
                &v.text("text").map_err(failed)?,
                v.get("height").map_err(failed)?,
                false,
            )
            .map_err(failed)?;
            let mut b = Builder::default();
            extrude(
                &mut b,
                &region.polygons,
                0.0,
                v.get("thickness").map_err(failed)?,
            )
            .map_err(failed)?;
            b.finish().map_err(failed)
        }
        "mold" => mold(v, control),
        "whistle" => whistle(v, control),
        _ => Err(failed(format!("The part library has no item \"{id}\"."))),
    }
}

fn mold(v: &Values, control: &mut Control) -> RepairResult<Mesh> {
    let failed = RepairError::Failed;
    let height = v.get("height").map_err(failed)?;
    let depth = v.get("depth").map_err(failed)?;
    let thickness = v.get("thickness").map_err(failed)?;
    let margin = v.get("margin").map_err(failed)?;
    let raised = v.choice("style").map_err(failed)? == "raised";
    if !raised && depth >= thickness - 0.1 {
        return Err(failed(
            "The letters must be less deep than the plate is thick.".into(),
        ));
    }
    let (region, width) = caption(
        &v.text("text").map_err(failed)?,
        height,
        v.toggle("mirror").map_err(failed)?,
    )
    .map_err(failed)?;
    let (w, h) = (width / 2.0 + margin, height / 2.0 + margin);
    let plate = make_box(
        [-w as f32, -h as f32, 0.0],
        [w as f32, h as f32, thickness as f32],
    );
    let frame = Frame {
        origin: [0.0, 0.0, thickness as f32 as f64],
        u: [1.0, 0.0, 0.0],
        v: [0.0, 1.0, 0.0],
        w: [0.0, 0.0, 1.0],
    };
    label::emboss_region(&plate, &frame, &region, raised, depth, control)?.map_err(failed)
}

/// Whistle dimensions at 100 %, mm: x along the whistle, the mouthpiece at −x, z up.
const CHAMBER_R: f64 = 9.0;
const CHAMBER_WALL: f64 = 2.0;
const HALF_WIDTH: f64 = 7.0;
const MOUTH_START: f64 = -32.0;
const MOUTH_BOTTOM: f64 = 11.0;

/// Signed distance to a box (negative inside).
fn sdf_box(p: [f64; 3], lo: [f64; 3], hi: [f64; 3]) -> f64 {
    let q: [f64; 3] = [0, 1, 2].map(|k| (lo[k] - p[k]).max(p[k] - hi[k]));
    let outside = q.iter().map(|x| x.max(0.0).powi(2)).sum::<f64>().sqrt();
    outside + q.iter().copied().fold(f64::NEG_INFINITY, f64::max).min(0.0)
}

/// Signed distance to a cylinder along y, centred at (0, 0, `cz`).
fn sdf_cylinder_y(p: [f64; 3], cz: f64, r: f64, half: f64) -> f64 {
    let radial = p[0].hypot(p[2] - cz) - r;
    let along = p[1].abs() - half;
    let outside = radial.max(0.0).hypot(along.max(0.0));
    outside + radial.max(along).min(0.0)
}

/// The whistle's solid, negative inside: a round chamber, a mouthpiece on top of it with a thin
/// windway that blows across a window onto the chamber wall's edge.
fn whistle_shape(p: [f64; 3]) -> f64 {
    let cz = CHAMBER_R;
    let top = 2.0 * CHAMBER_R;
    let body = sdf_cylinder_y(p, cz, CHAMBER_R, HALF_WIDTH).min(sdf_box(
        p,
        [MOUTH_START, -HALF_WIDTH, MOUTH_BOTTOM],
        [0.0, HALF_WIDTH, top],
    ));
    let cavity = sdf_cylinder_y(p, cz, CHAMBER_R - CHAMBER_WALL, HALF_WIDTH - CHAMBER_WALL);
    let inner = HALF_WIDTH - CHAMBER_WALL;
    let windway = sdf_box(
        p,
        [MOUTH_START - 1.0, -inner, top - 3.4],
        [-1.5, inner, top - 2.2],
    );
    let window = sdf_box(p, [-1.5, -inner, top - 4.5], [3.5, inner, top + 1.0]);
    body.max(-cavity).max(-windway).max(-window)
}

fn whistle(v: &Values, control: &mut Control) -> RepairResult<Mesh> {
    let failed = RepairError::Failed;
    let s = v.get("scale").map_err(failed)? / 100.0;
    let cell = if v.preview { 0.5 } else { 0.25 } * s;
    let lo = [MOUTH_START * s - 1.0, -HALF_WIDTH * s - 1.0, -1.0];
    let hi = [
        CHAMBER_R * s + 1.0,
        HALF_WIDTH * s + 1.0,
        2.0 * CHAMBER_R * s + 1.0,
    ];
    let body = field(
        lo,
        hi,
        cell,
        |p| whistle_shape(p.map(|x| x / s)) * s,
        &mut control.sub(0.0, 0.7),
    )?
    .ok_or_else(|| failed("Those values leave nothing to make.".into()))?;
    let text = v.text("text").map_err(failed)?;
    if text.trim().is_empty() {
        return recentred(body);
    }
    // The caption on the mouthpiece's side (−y), raised, as high as the side allows.
    let (room_w, room_h) = (
        (-MOUTH_START - 6.0) * s,
        (2.0 * CHAMBER_R - MOUTH_BOTTOM - 2.5) * s,
    );
    let shape = label::text_shape(BUNDLED, 0, &text, FONT_SIZE).map_err(failed)?;
    let height = room_h.min(room_w / shape.aspect);
    let region = label::sized(&shape, height, height * shape.aspect, false);
    let frame = Frame {
        origin: [
            (MOUTH_START + 3.0) * s + room_w / 2.0,
            -HALF_WIDTH * s,
            (MOUTH_BOTTOM + 2.0 * CHAMBER_R) * s / 2.0,
        ],
        u: [1.0, 0.0, 0.0],
        v: [0.0, 0.0, 1.0],
        w: [0.0, -1.0, 0.0],
    };
    let labelled = label::emboss_region(
        &body,
        &frame,
        &region,
        true,
        0.6 * s,
        &mut control.sub(0.7, 1.0),
    )?
    .map_err(failed)?;
    recentred(labelled)
}

fn recentred(mesh: Mesh) -> RepairResult<Mesh> {
    let mut b = Builder::default();
    b.append(&mesh);
    b.finish().map_err(RepairError::Failed)
}
