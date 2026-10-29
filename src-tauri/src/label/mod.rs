//! Labels (Home and Modify › Generate label): text, a picture, text from a list and text on a
//! breakaway tag, raised from a part's surface or sunk into it, mirrored or not. The window places
//! each label (a point on the part, the surface normal there, a turn, a size); this module turns
//! its text or picture into a flat shape (`region`) and puts that into the part (`emboss`), so the
//! labelled part is closed, oriented and free of crossings wherever the part was.
//!
//! Several labels, of any kinds, go on in one run, one after the other. List labels make one
//! copy of the part per entry, as many as the shortest list has.

mod emboss;
pub mod fit;
pub mod font;
pub mod list;
mod picture;
pub(crate) mod region;
mod tag;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

pub(crate) use emboss::Frame;
pub use font::{BUNDLED_ID, FontInfo, FontSource};
pub(crate) use region::Region;

use crate::library::Picture;
use crate::mesh::Mesh;
use crate::repair::{Control, RepairResult};
use emboss::Relief;

/// Largest label side, mm.
pub const MAX_SIZE: f64 = 2000.0;
/// Copies a list may make at most.
pub const MAX_COPIES: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Text,
    Image,
    List,
    Tag,
}

/// Raised or sunk, and whether the shape is mirrored (for moulds and stamps).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Build {
    Add,
    AddMirrored,
    Subtract,
    SubtractMirrored,
}

impl Build {
    fn mirrored(self) -> bool {
        matches!(self, Build::AddMirrored | Build::SubtractMirrored)
    }

    fn raised(self) -> bool {
        matches!(self, Build::Add | Build::AddMirrored)
    }
}

/// A placed label, as src/label/types.ts sends it. Lengths in mm, the angle in degrees.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spec {
    pub kind: Kind,
    /// The caption (Text and Tag).
    #[serde(default)]
    pub text: String,
    /// One caption per copy (List).
    #[serde(default)]
    pub entries: Vec<String>,
    /// A font id from `label_fonts`.
    #[serde(default = "bundled")]
    pub font: String,
    /// The font size in points: how finely the letters' curves are cut, not how big they are.
    #[serde(default = "default_font_size")]
    pub font_size: f64,
    pub height: f64,
    /// Used when `keep_aspect` is off.
    #[serde(default)]
    pub width: f64,
    #[serde(default = "yes")]
    pub keep_aspect: bool,
    pub depth: f64,
    pub build: Build,
    /// The tag plate's thickness (Tag).
    #[serde(default)]
    pub thickness: f64,
    /// Where the label's centre is, world space.
    pub point: [f64; 3],
    /// The surface normal there, world space.
    pub normal: [f64; 3],
    /// The turn about the normal, anticlockwise seen from outside.
    #[serde(default)]
    pub angle: f64,
    /// The direction the tag plate stands out in (Tag); the normal when missing.
    #[serde(default)]
    pub tag_direction: Option<[f64; 3]>,
    /// Grey level below which a picture's pixels are part of the label (Image), 0 to 1.
    #[serde(default = "half")]
    pub threshold: f64,
}

fn bundled() -> String {
    BUNDLED_ID.into()
}

fn default_font_size() -> f64 {
    48.0
}

fn yes() -> bool {
    true
}

fn half() -> f64 {
    0.5
}

/// A label's flat shape at unit height, centred, and its width ÷ height. For text the frame is
/// the letters' extent; for a picture, the picture.
#[derive(Clone, Debug)]
pub struct Shape {
    pub(crate) region: Region,
    pub aspect: f64,
    /// How many source units (font units, picture samples) one unit of `region` is.
    pub unit: f64,
}

/// What the window draws while a label is placed: `Shape` as triangles and outline segments,
/// x, y per point, in units of the label's height.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeReply {
    pub aspect: f64,
    pub fill: Vec<f32>,
    pub outline: Vec<f32>,
}

impl Shape {
    pub fn reply(&self) -> Result<ShapeReply, String> {
        let points: Vec<[f64; 2]> = self.region.loops().flatten().copied().collect();
        let mut fill = Vec::new();
        for t in self.region.triangles()? {
            for i in t {
                fill.extend([points[i][0] as f32, points[i][1] as f32]);
            }
        }
        let mut outline = Vec::new();
        for lp in self.region.loops() {
            for k in 0..lp.len() {
                let (a, b) = (lp[k], lp[(k + 1) % lp.len()]);
                outline.extend([a[0] as f32, a[1] as f32, b[0] as f32, b[1] as f32]);
            }
        }
        Ok(ShapeReply {
            aspect: self.aspect,
            fill,
            outline,
        })
    }
}

/// `text` in the font at unit height. Fails with a sentence for an empty caption or letters the
/// font lacks.
pub fn text_shape(font: &[u8], index: u32, text: &str, size: f64) -> Result<Shape, String> {
    if text.trim().is_empty() {
        return Err("Type a caption first.".into());
    }
    let contours = font::text_contours(font, index, text, size)?;
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for p in contours.iter().flatten() {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let height = hi[1] - lo[1];
    if contours.is_empty() || height <= 0.0 || hi[0] <= lo[0] {
        return Err("The caption has no letters to raise.".into());
    }
    let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let unit: Vec<Vec<[f64; 2]>> = contours
        .iter()
        .map(|c| {
            c.iter()
                .map(|p| [(p[0] - centre[0]) / height, (p[1] - centre[1]) / height])
                .collect()
        })
        .collect();
    Ok(Shape {
        region: Region::from_contours(&unit)?,
        aspect: (hi[0] - lo[0]) / height,
        unit: height,
    })
}

/// Every entry of a list in the font, all at one scale: that of the widest entry, which sets the
/// label's size, so the letters of every copy are the same size. Each entry is centred on the
/// label; `aspect` is the widest entry's, so the same height and width size every copy.
pub fn list_shapes(
    font: &[u8],
    index: u32,
    entries: &[String],
    size: f64,
) -> Result<Vec<Shape>, String> {
    let shapes: Vec<Shape> = entries
        .iter()
        .map(|e| text_shape(font, index, e, size))
        .collect::<Result<_, _>>()?;
    let Some(widest) = shapes
        .iter()
        .max_by(|a, b| (a.aspect * a.unit).total_cmp(&(b.aspect * b.unit)))
    else {
        return Err("The list is empty. Make a list or load a CSV file first.".into());
    };
    let (unit, aspect) = (widest.unit, widest.aspect);
    Ok(shapes
        .iter()
        .map(|s| {
            let f = s.unit / unit;
            Shape {
                region: s.region.mapped(|p| [p[0] * f, p[1] * f], false),
                aspect,
                unit,
            }
        })
        .collect())
}

/// The shape a label is sized by: its caption, a list's widest entry, or its picture (a square
/// before one is picked, when `square_without_picture`).
pub fn sizing_shape(
    kind: Kind,
    text: &str,
    entries: &[String],
    font: &dyn Fn() -> Result<(Vec<u8>, u32), String>,
    size: f64,
    picture: Option<&Picture>,
    threshold: f64,
) -> Result<Shape, String> {
    match kind {
        Kind::Image => match picture {
            Some(p) => picture_shape(p, threshold),
            None => Err("Pick a picture first: click the picture area.".into()),
        },
        Kind::List => {
            let (data, index) = font()?;
            let shapes = list_shapes(&data, index, entries, size)?;
            // The widest entry, which keeps its own outline at the shared scale.
            Ok(shapes
                .into_iter()
                .max_by(|a, b| region_width(&a.region).total_cmp(&region_width(&b.region)))
                .expect("list_shapes returns one shape per entry"))
        }
        _ => {
            let (data, index) = font()?;
            text_shape(&data, index, text, size)
        }
    }
}

fn region_width(region: &Region) -> f64 {
    region.bounds().map_or(0.0, |(lo, hi)| hi[0] - lo[0])
}

/// The picture's parts darker than `threshold`, the picture at unit height.
pub fn picture_shape(picture: &Picture, threshold: f64) -> Result<Shape, String> {
    if picture.width == 0 || picture.height == 0 {
        return Err("Pick a picture first: click the picture area.".into());
    }
    let small = picture::shrunk(picture);
    let (w, h) = (small.width as f64, small.height as f64);
    let contours: Vec<Vec<[f64; 2]>> = picture::dark_contours(&small, threshold)
        .into_iter()
        .map(|c| {
            c.iter()
                .map(|p| [(p[0] - w / 2.0) / h, (p[1] - h / 2.0) / h])
                .collect()
        })
        .collect();
    let region = Region::from_contours(&contours)?;
    if region.is_empty() {
        return Err(
            "Nothing of the picture is darker than the threshold. Move Threshold to the right."
                .into(),
        );
    }
    Ok(Shape {
        region,
        aspect: w / h,
        unit: h,
    })
}

/// The label's width in mm: from its height and shape, or as typed.
pub fn width_of(spec: &Spec, aspect: f64) -> f64 {
    if spec.keep_aspect {
        spec.height * aspect
    } else {
        spec.width
    }
}

/// Shortest outline side, mm: the walls' edges stay longer than Repair's degenerate tolerance.
const SHORTEST_SIDE: f64 = 0.005;

/// The shape at the label's size in mm, mirrored when the build type says so.
pub(crate) fn sized(shape: &Shape, height: f64, width: f64, mirrored: bool) -> Region {
    let sx = width / shape.aspect.max(1e-12) * if mirrored { -1.0 } else { 1.0 };
    shape
        .region
        .mapped(|p| [p[0] * sx, p[1] * height], mirrored)
        .without_short_sides(SHORTEST_SIDE)
}

/// Checks a label's numbers; a sentence names the first one that is wrong.
fn check(spec: &Spec, aspect: f64) -> Result<(), String> {
    let ok = |x: f64| x.is_finite() && x > 0.0 && x <= MAX_SIZE;
    if !ok(spec.height) {
        return Err(format!(
            "Height must be more than 0 and at most {MAX_SIZE} mm."
        ));
    }
    if !ok(width_of(spec, aspect)) {
        return Err(format!(
            "Width must be more than 0 and at most {MAX_SIZE} mm."
        ));
    }
    if !ok(spec.depth) {
        return Err(format!(
            "Depth must be more than 0 and at most {MAX_SIZE} mm."
        ));
    }
    if spec.kind == Kind::Tag && !ok(spec.thickness) {
        return Err("Thickness must be more than 0.".into());
    }
    if spec.kind == Kind::Tag && !spec.build.raised() && spec.depth >= spec.thickness {
        return Err("A sunk caption must be less deep than the tag is thick.".into());
    }
    if !(spec.point.iter().chain(&spec.normal).all(|x| x.is_finite()) && spec.angle.is_finite()) {
        return Err("The label's place is damaged; place it again.".into());
    }
    Ok(())
}

/// A font's bytes and face index, by font id.
pub type FontData<'a> = &'a dyn Fn(&str) -> Result<(Vec<u8>, u32), String>;

/// What a label's text or picture is made from.
pub struct Sources<'a> {
    /// The font's bytes and face index, for the labels' `font` ids.
    pub font: FontData<'a>,
    /// The picture of each label (by position in the list), for Image labels.
    pub pictures: &'a [Option<Picture>],
}

/// The labelled part, or one per list entry.
pub struct Applied {
    /// Each copy's mesh (world space) with its list entries joined, or "" without a list.
    pub copies: Vec<(String, Mesh)>,
    /// Said to the user: lists of different lengths made as many copies as the shortest.
    pub note: Option<String>,
}

/// Every label in `specs` put on `mesh` (world space), in order. With list labels, one copy per
/// entry. Errors with a sentence naming the label (1 = the first) that cannot go where it is.
pub fn apply(
    mesh: &Mesh,
    specs: &[Spec],
    sources: &Sources,
    control: &mut Control,
) -> RepairResult<Result<Applied, String>> {
    if specs.is_empty() {
        return Ok(Err("Place a label first: click on the part.".into()));
    }
    let lists: Vec<usize> = specs
        .iter()
        .filter(|s| s.kind == Kind::List)
        .map(|s| s.entries.len())
        .collect();
    let copies = lists.iter().copied().min().unwrap_or(1);
    if copies == 0 {
        return Ok(Err(
            "The list is empty. Make a list or load a CSV file first.".into(),
        ));
    }
    if copies > MAX_COPIES {
        return Ok(Err(format!("A list may make at most {MAX_COPIES} copies.")));
    }
    let note = (lists.len() > 1 && lists.iter().any(|&n| n != copies)).then(|| {
        format!("The lists have different lengths, so {copies} copies were made, as many as the shortest list has entries.")
    });
    // Every shape is made once: one per label, or one per entry of a list, all at its scale.
    let mut shapes: Vec<Vec<Shape>> = Vec::with_capacity(specs.len());
    for (i, spec) in specs.iter().enumerate() {
        let made = match spec.kind {
            Kind::List => (sources.font)(&spec.font).and_then(|(data, index)| {
                list_shapes(&data, index, &spec.entries[..copies], spec.font_size)
            }),
            _ => shape_for(spec, &spec.text, i, sources).map(|s| vec![s]),
        };
        match made {
            Ok(s) => shapes.push(s),
            Err(why) => return Ok(Err(numbered(specs.len(), i, &why))),
        }
    }
    let mut out = Vec::with_capacity(copies);
    let steps = (copies * specs.len()) as f64;
    for copy in 0..copies {
        let mut current = mesh.clone();
        let mut names = Vec::new();
        for (i, spec) in specs.iter().enumerate() {
            let step = (copy * specs.len() + i) as f64;
            let mut sub = control.sub(step / steps, (step + 1.0) / steps);
            let shape = if spec.kind == Kind::List {
                names.push(spec.entries[copy].trim().to_string());
                &shapes[i][copy]
            } else {
                &shapes[i][0]
            };
            if let Err(why) = check(spec, shape.aspect) {
                return Ok(Err(numbered(specs.len(), i, &why)));
            }
            match place(&current, spec, shape, &mut sub)? {
                Ok(m) => current = m,
                Err(why) => return Ok(Err(numbered(specs.len(), i, &why))),
            }
        }
        out.push((names.join(" "), current));
    }
    control.progress(1.0);
    Ok(Ok(Applied { copies: out, note }))
}

fn numbered(count: usize, i: usize, why: &str) -> String {
    if count == 1 {
        why.to_string()
    } else {
        format!("Label {}: {why}", i + 1)
    }
}

/// The shape of label `i` with `text`.
pub fn shape_for(spec: &Spec, text: &str, i: usize, sources: &Sources) -> Result<Shape, String> {
    match spec.kind {
        Kind::Image => match sources.pictures.get(i).and_then(|p| p.as_ref()) {
            Some(p) => picture_shape(p, spec.threshold),
            None => Err("Pick a picture first: click the picture area.".into()),
        },
        _ => {
            let (data, index) = (sources.font)(&spec.font)?;
            text_shape(&data, index, text, spec.font_size)
        }
    }
}

/// One label put on `mesh`.
fn place(
    mesh: &Mesh,
    spec: &Spec,
    shape: &Shape,
    control: &mut Control,
) -> RepairResult<Result<Mesh, String>> {
    let Some(frame) = Frame::label(spec.point, spec.normal, spec.angle.to_radians()) else {
        return Ok(Err("The label's place is damaged; place it again.".into()));
    };
    let width = width_of(spec, shape.aspect);
    let region = sized(shape, spec.height, width, spec.build.mirrored());
    if spec.kind == Kind::Tag {
        return tag::attach(mesh, &frame, spec, &region, width, control);
    }
    let relief = if spec.build.raised() {
        Relief::Raise(spec.depth)
    } else {
        Relief::Sink(spec.depth)
    };
    Ok(emboss::emboss(mesh, &frame, &region, relief, control)?.map(|e| e.mesh))
}

/// `region` (mm, in the frame's plane) raised or sunk into `mesh`, for the library's moulds.
pub(crate) fn emboss_region(
    mesh: &Mesh,
    frame: &Frame,
    region: &Region,
    raised: bool,
    depth: f64,
    control: &mut Control,
) -> RepairResult<Result<Mesh, String>> {
    let relief = if raised {
        Relief::Raise(depth)
    } else {
        Relief::Sink(depth)
    };
    Ok(emboss::emboss(mesh, frame, region, relief, control)?.map(|e| e.mesh))
}
