//! Part library › Design: grids, pictures as reliefs, QR codes, screws and nuts, gears, vases,
//! picture sweeps, bracelets, bricks and base plates.

use std::collections::{HashMap, HashSet};
use std::f64::consts::{PI, TAU};
use std::hash::Hash;

use super::build::{Builder, Cap, P, P2, area2, circle_2d, extrude, heightfield, steps};
use super::gear::{Gear, gear_mesh};
use super::solid::union;
use super::terrace::terrace;
use super::threads::{Head, metric, nut, screw};
use super::{
    Category, Choice, Item, Kind, Param, Picture, Polygon, Sources, Values, choice, count, image,
    item, mm, num, text, toggle,
};
use crate::mesh::{Mesh, bounds};
use crate::repair::{Control, RepairError, RepairResult};

fn design(
    id: &'static str,
    label: &'static str,
    description: &'static str,
    params: Vec<Param>,
) -> Item {
    item(id, label, Category::Design, description, params)
}

fn sizes() -> Vec<(&'static str, &'static str)> {
    super::METRIC.iter().map(|m| (m.name, m.name)).collect()
}

fn metric_choice() -> Param {
    let mut p = choice("thread", "Metric thread", "M6", &[]);
    if let Kind::Choice { options, .. } = &mut p.kind {
        *options = sizes()
            .into_iter()
            .map(|(value, label)| Choice {
                value,
                label,
                soon: false,
            })
            .collect();
    }
    p.help("The thread size; the head and nut sizes follow the ISO standards for it.")
}

pub(super) fn catalog() -> Vec<Item> {
    let mut qr_kind = choice(
        "code",
        "Code type",
        "qr",
        &[
            ("qr", "QR code"),
            ("datamatrix", "Data Matrix"),
            ("aztec", "Aztec code"),
        ],
    );
    if let Kind::Choice { options, .. } = &mut qr_kind.kind {
        for o in options.iter_mut().skip(1) {
            o.soon = true;
        }
    }
    vec![
        design(
            "rect_grid",
            "Rectangle grid",
            "A flat plate with a grid of rectangular holes.",
            vec![
                mm("length", "Length (X)", 60.0),
                mm("width", "Width (Y)", 40.0),
                mm("height", "Height (Z)", 3.0),
                count("holes_x", "Holes in X", 6.0, 1.0, 200.0),
                count("holes_y", "Holes in Y", 4.0, 1.0, 200.0),
                mm("wall", "Wall thickness", 2.0)
                    .help("The width of the bars between the holes and of the frame."),
            ],
        ),
        design(
            "hex_grid",
            "Hexagonal grid",
            "A honeycomb plate of hexagonal holes.",
            vec![
                mm("hole", "Hole width", 8.0).help("Corner to corner across each hexagonal hole."),
                mm("wall", "Wall thickness", 1.5),
                count("holes_x", "Holes in X", 6.0, 1.0, 100.0),
                count("holes_y", "Holes in Y", 5.0, 1.0, 100.0),
                mm("height", "Height (Z)", 3.0),
            ],
        ),
        design(
            "hex_cylinder",
            "Cylinder with hexagon grid",
            "A tube whose wall is a honeycomb of hexagonal holes.",
            vec![
                mm("radius", "Radius", 15.0),
                mm("height", "Height (Z)", 40.0),
                mm("wall", "Wall thickness", 2.0),
                count("around", "Holes around", 12.0, 6.0, 200.0),
                mm("bars", "Thickness of bars", 1.5)
                    .help("The width of the bars between two holes."),
            ],
        ),
        design(
            "square_grid",
            "Square grid",
            "A flat mesh of square holes.",
            vec![
                mm("cell", "Cell size", 5.0).help("The side of each square hole."),
                mm("wall", "Wall thickness", 1.0),
                count("holes_x", "Holes in X", 8.0, 1.0, 300.0),
                count("holes_y", "Holes in Y", 8.0, 1.0, 300.0),
                mm("height", "Height (Z)", 2.0),
            ],
        ),
        design(
            "bitmap",
            "Bitmap",
            "A picture as a relief in steps of grey: raised from a plate, or cut into a block.",
            vec![
                image("image", "Image"),
                mm("width", "Width (X)", 60.0),
                toggle("keep_aspect", "Keep aspect ratio", true),
                mm("depth", "Depth (Y)", 40.0).when("!keep_aspect"),
                choice(
                    "relief",
                    "Relief",
                    "raised",
                    &[("raised", "Raised"), ("cut", "Cut in")],
                ),
                mm("base", "Base thickness", 2.0),
                mm("relief_height", "Relief height", 2.0),
                count("levels", "Levels of grey", 2.0, 2.0, 250.0)
                    .help("How many heights the greys are sorted into."),
                toggle("invert", "Invert colours", false)
                    .help("By default dark parts stand highest (or are cut deepest)."),
                count("detail", "Level of detail", 200.0, 16.0, 1000.0)
                    .help("Samples along the longer side."),
            ],
        ),
        design(
            "heightmap",
            "Heightmap",
            "A picture as a smooth height field: the darker, the higher.",
            vec![
                image("image", "Image"),
                mm("width", "Width (X)", 60.0),
                toggle("keep_aspect", "Keep aspect ratio", true),
                mm("depth", "Depth (Y)", 40.0).when("!keep_aspect"),
                mm("base", "Base thickness", 1.0),
                mm("height", "Height (Z)", 8.0).help("Height of the darkest point above the base."),
                toggle("invert", "Invert colours", false),
                count("detail", "Level of detail", 150.0, 8.0, 1000.0)
                    .help("Samples along the longer side."),
            ],
        ),
        design(
            "qr_code",
            "QR Code",
            "A QR code standing on a plate.",
            vec![
                qr_kind,
                text("data", "Text", "https://example.com", 1000)
                    .help("What the code says, up to 1000 characters."),
                mm("size", "Size", 40.0)
                    .help("The edge length of the square plate, quiet zone included."),
                mm("base", "Plate thickness", 2.0),
                mm("module_height", "Raised by", 1.0),
                choice(
                    "ecc",
                    "Error correction",
                    "medium",
                    &[
                        ("low", "Low (7 %)"),
                        ("medium", "Medium (15 %)"),
                        ("quartile", "Quartile (25 %)"),
                        ("high", "High (30 %)"),
                    ],
                ),
                count("quiet", "Quiet zone (modules)", 2.0, 0.0, 10.0),
            ],
        ),
        design("text", "Text", "Letters as a solid.", Vec::new())
            .soon("it needs the text rendering that comes with labels."),
        design(
            "screw",
            "Screw",
            "A metric screw with an ISO thread and a choice of heads.",
            vec![
                metric_choice(),
                mm("length", "Thread length", 16.0).help("The threaded length, without the head."),
                choice(
                    "head",
                    "Kind of head",
                    "socket",
                    &[
                        ("socket", "Hexagon socket"),
                        ("hexagon", "Hexagon head"),
                        ("slotted", "Slotted"),
                    ],
                ),
                count("resolution", "Points per turn", 48.0, 24.0, 240.0),
            ],
        ),
        design(
            "nut",
            "Nut",
            "A hexagon nut for the metric screw of the same size.",
            vec![
                metric_choice(),
                num("clearance", "Clearance to screw", "mm", 0.2, 0.0, 2.0)
                    .help("How much wider than the screw's thread the nut's is, all round."),
                count("resolution", "Points per turn", 48.0, 24.0, 240.0),
            ],
        ),
        design(
            "gear",
            "Gear wheel",
            "A spur gear with involute teeth.",
            vec![
                count("teeth", "Number of teeth", 20.0, 6.0, 400.0),
                num("module", "Module", "mm", 2.0, 0.2, 50.0)
                    .help("Pitch diameter ÷ teeth; gears that mesh share it."),
                mm("thickness", "Thickness", 8.0),
                num("bore", "Bore diameter", "mm", 5.0, 0.0, 1000.0),
                num("pressure", "Pressure angle", "°", 20.0, 14.5, 30.0),
                num("backlash", "Backlash", "mm", 0.1, 0.0, 2.0),
            ],
        ),
        design(
            "light_gear",
            "Light gear wheel",
            "A spur gear with round holes between hub and rim to save material.",
            vec![
                count("teeth", "Number of teeth", 40.0, 12.0, 400.0),
                num("module", "Module", "mm", 2.0, 0.2, 50.0)
                    .help("Pitch diameter ÷ teeth; gears that mesh share it."),
                mm("thickness", "Thickness", 6.0),
                num("bore", "Bore diameter", "mm", 8.0, 0.0, 1000.0),
                count("holes", "Lightening holes", 6.0, 2.0, 24.0),
                num("pressure", "Pressure angle", "°", 20.0, 14.5, 30.0),
                num("backlash", "Backlash", "mm", 0.1, 0.0, 2.0),
            ],
        ),
        design("whistle", "Whistle", "A whistle with a label.", Vec::new())
            .soon("its label needs the text rendering that comes with labels."),
        design("mold", "Mold", "A mould with a label.", Vec::new())
            .soon("its label needs the text rendering that comes with labels."),
        design(
            "vase",
            "Vase",
            "A vase turned from a profile, optionally twisted, with handles.",
            vec![
                choice(
                    "profile",
                    "Profile",
                    "classic",
                    &[
                        ("classic", "Classic"),
                        ("bulb", "Bulb"),
                        ("tulip", "Tulip"),
                        ("cylinder", "Straight"),
                    ],
                ),
                mm("height", "Height (Z)", 80.0),
                mm("width", "Widest diameter", 60.0),
                mm("wall", "Wall thickness", 2.0),
                count("lobes", "Ribs", 8.0, 0.0, 32.0)
                    .help("Ribs round the vase; 0 keeps it round."),
                num("turns", "Turns", "", 0.25, -4.0, 4.0)
                    .help("How far the ribs twist from bottom to top."),
                count("handles", "Handles", 2.0, 0.0, 4.0),
                count("segments", "Segments", 96.0, 24.0, 720.0),
            ],
        ),
        design(
            "picture_sweep",
            "Picture sweep",
            "The outline of a picture swept along a path, turning as it goes.",
            vec![
                image("image", "Image"),
                mm("size", "Picture size", 30.0).help("The longer side of the picture's outline."),
                choice(
                    "path",
                    "Path",
                    "straight",
                    &[("straight", "Straight up"), ("arc", "Quarter circle")],
                ),
                mm("length", "Path length", 40.0),
                num("spin", "Spin", "°", 90.0, -1440.0, 1440.0)
                    .help("How far the outline turns from start to end."),
                count("steps", "Steps", 40.0, 1.0, 1000.0),
                toggle("invert", "Invert colours", false)
                    .help("By default the dark parts make the outline."),
            ],
        ),
        design(
            "bracelet",
            "Bracelet",
            "A bangle sized to the wrist.",
            vec![
                choice(
                    "style",
                    "Style",
                    "smooth",
                    &[
                        ("smooth", "Smooth band"),
                        ("wave", "Wave"),
                        ("twisted", "Twisted"),
                    ],
                ),
                mm("wrist", "Wrist circumference", 170.0),
                choice(
                    "fit",
                    "Fit",
                    "balanced",
                    &[
                        ("close", "Close"),
                        ("balanced", "Balanced"),
                        ("loose", "Loose"),
                    ],
                ),
                mm("band_width", "Band width", 12.0),
                mm("thickness", "Thickness", 3.0),
                count("around", "Horizontal mesh density", 180.0, 24.0, 2000.0),
                count("across", "Vertical mesh density", 16.0, 6.0, 200.0),
            ],
        ),
        design(
            "brick",
            "Brick",
            "A stud-style building brick, its size counted in studs.",
            vec![
                count("studs_x", "Length in studs", 4.0, 1.0, 48.0),
                count("studs_y", "Width in studs", 2.0, 1.0, 48.0),
                choice(
                    "kind",
                    "Height",
                    "brick",
                    &[("brick", "Brick (9.6 mm)"), ("plate", "Plate (3.2 mm)")],
                ),
                toggle("hollow", "Hollow underside", true),
            ],
        ),
        design(
            "mesh_on_spline",
            "Mesh on spline",
            "A mesh bent along a curve.",
            Vec::new(),
        )
        .soon("its behaviour is not documented well enough to build it."),
        design(
            "base_plate",
            "Base plate",
            "A plate under the selected part, shaped like its shadow.",
            vec![
                choice(
                    "type",
                    "Baseplate type",
                    "solid",
                    &[
                        ("solid", "Solid"),
                        ("hexagon", "Hexagonal cells"),
                        ("square", "Square cells"),
                    ],
                ),
                mm("thickness", "Baseplate height", 2.0)
                    .help("The plate's thickness; it ends where the part's lowest point is."),
                mm("cell", "Cell size", 5.0).help("The width of each hole in a structured plate."),
                mm("wall", "Wall thickness", 1.2)
                    .help("The walls between the holes, and the plate's rim."),
                num("offset", "Offset of shadow", "mm", 2.0, 0.0, 1000.0)
                    .help("A margin added round the shadow."),
                toggle("outer_only", "Use only outer contours", false)
                    .help("Fill in areas the shadow surrounds completely."),
                toggle("invert", "Invert baseplate", false).help(
                    "Make the plate round the shadow instead: its box, padded, without the shadow.",
                ),
                num(
                    "outer_offset",
                    "Outer offset of inverted baseplate",
                    "mm",
                    5.0,
                    0.0,
                    1000.0,
                )
                .when("invert"),
                toggle("use_lower", "Use lower limit", false),
                num("lower", "Lower limit", "mm", 0.0, -MAX_LIMIT, MAX_LIMIT)
                    .help("Take the shadow only from the part above this height.")
                    .when("use_lower"),
                toggle("use_upper", "Use upper limit", false),
                num("upper", "Upper limit", "mm", 10.0, -MAX_LIMIT, MAX_LIMIT)
                    .help("Take the shadow only from the part below this height.")
                    .when("use_upper"),
            ],
        )
        .made_from_parts(),
    ]
}

const MAX_LIMIT: f64 = 10_000.0;

pub(super) fn make(
    id: &str,
    v: &Values,
    sources: &Sources,
    control: &mut Control,
) -> RepairResult<Mesh> {
    let failed = RepairError::Failed;
    match id {
        "rect_grid" => rect_grid(v).map_err(failed),
        "square_grid" => square_grid(v).map_err(failed),
        "hex_grid" => hex_grid(v).map_err(failed),
        "hex_cylinder" => hex_cylinder(v).map_err(failed),
        "bitmap" => bitmap(v, sources.picture.as_ref()).map_err(failed),
        "heightmap" => heightmap(v, sources.picture.as_ref()).map_err(failed),
        "qr_code" => qr_code(v).map_err(failed),
        "screw" => {
            let size = metric(v.choice("thread").map_err(failed)?);
            let head = match v.choice("head").map_err(failed)? {
                "hexagon" => Head::Hexagon,
                "slotted" => Head::Slotted,
                _ => Head::Socket,
            };
            let length = v.get("length").map_err(failed)?;
            let mesh = screw(
                size,
                length,
                head,
                v.count("resolution").map_err(failed)?,
                control,
            )?;
            recentre(mesh)
        }
        "nut" => {
            let size = metric(v.choice("thread").map_err(failed)?);
            nut(
                size,
                v.get("clearance").map_err(failed)?,
                v.count("resolution").map_err(failed)?,
            )
            .and_then(recentre_plain)
            .map_err(failed)
        }
        "gear" => gear(v, 0).map_err(failed),
        "light_gear" => light_gear(v).map_err(failed),
        "vase" => vase(v, control),
        "picture_sweep" => picture_sweep(v, sources.picture.as_ref()).map_err(failed),
        "bracelet" => bracelet(v).map_err(failed),
        "brick" => brick(v, control),
        "base_plate" => base_plate(v, sources, control),
        _ => unreachable!("every design item has a generator"),
    }
}

/// Stands a mesh centred on the Z axis on z = 0 (a mesh that already faces outwards).
fn recentre(mesh: Mesh) -> RepairResult<Mesh> {
    recentre_plain(mesh).map_err(RepairError::Failed)
}

fn recentre_plain(mesh: Mesh) -> Result<Mesh, String> {
    let mut b = Builder::default();
    b.append(&mesh);
    b.finish()
}

// ---------------------------------------------------------------------------------------------
// Plates: flat faces between two layers

/// A plate of convex faces (each anticlockwise in its own 2D coordinates, given by point keys) between
/// a low and a high layer: each face top and bottom, and a wall along every edge only one face has.
/// `map` places a 2D point on a layer. Used flat (grids) and wrapped round a cylinder.
fn plate<K: Copy + Hash + Eq>(
    faces: &[Vec<K>],
    at: impl Fn(K) -> P2,
    map: impl Fn(P2, bool) -> P,
) -> Builder {
    let mut b = Builder::default();
    let mut ids: HashMap<(K, bool), u32> = HashMap::new();
    let mut id = |b: &mut Builder, k: K, high: bool| -> u32 {
        *ids.entry((k, high))
            .or_insert_with(|| b.point(map(at(k), high)))
    };
    let mut edges: HashSet<(K, K)> = HashSet::new();
    for face in faces {
        let n = face.len();
        for k in 0..n {
            edges.insert((face[k], face[(k + 1) % n]));
        }
    }
    for face in faces {
        let hi: Vec<u32> = face.iter().map(|&k| id(&mut b, k, true)).collect();
        let lo: Vec<u32> = face.iter().map(|&k| id(&mut b, k, false)).collect();
        for m in 1..face.len() - 1 {
            b.triangle(hi[0], hi[m], hi[m + 1]);
            b.triangle(lo[0], lo[m + 1], lo[m]);
        }
        let n = face.len();
        for k in 0..n {
            let (p, q) = (face[k], face[(k + 1) % n]);
            if !edges.contains(&(q, p)) {
                // The face is on the left of p → q; the wall faces right, outwards.
                let (pl, ql, qh, ph) = (
                    id(&mut b, p, false),
                    id(&mut b, q, false),
                    id(&mut b, q, true),
                    id(&mut b, p, true),
                );
                b.quad(pl, ql, qh, ph);
            }
        }
    }
    b
}

/// Solid cells of a grid of lines `xs` × `ys` as a plate from z0 to z1.
fn cells(
    xs: &[f64],
    ys: &[f64],
    solid: impl Fn(usize, usize) -> bool,
    z0: f64,
    z1: f64,
) -> Result<Mesh, String> {
    let mut faces = Vec::new();
    for j in 0..ys.len() - 1 {
        for i in 0..xs.len() - 1 {
            if solid(i, j) {
                faces.push(vec![(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)]);
            }
        }
    }
    plate(
        &faces,
        |(i, j)| [xs[i], ys[j]],
        |p, high| [p[0], p[1], if high { z1 } else { z0 }],
    )
    .finish()
}

/// Lines of a frame round `holes` holes of `hole` with bars of `wall`: wall, hole, wall, …, wall.
fn frame_lines(holes: usize, hole: f64, wall: f64) -> Vec<f64> {
    let mut out = vec![0.0];
    for _ in 0..holes {
        out.push(out[out.len() - 1] + wall);
        out.push(out[out.len() - 1] + hole);
    }
    out.push(out[out.len() - 1] + wall);
    out
}

fn rect_grid(v: &Values) -> Result<Mesh, String> {
    let (l, w, h) = (v.get("length")?, v.get("width")?, v.get("height")?);
    let (nx, ny, wall) = (v.count("holes_x")?, v.count("holes_y")?, v.get("wall")?);
    let hx = (l - (nx + 1) as f64 * wall) / nx as f64;
    let hy = (w - (ny + 1) as f64 * wall) / ny as f64;
    if hx <= 0.0 || hy <= 0.0 {
        return Err(
            "The walls take up the whole plate; make it bigger, or the walls thinner or fewer."
                .into(),
        );
    }
    let (xs, ys) = (frame_lines(nx, hx, wall), frame_lines(ny, hy, wall));
    cells(&xs, &ys, |i, j| i % 2 == 0 || j % 2 == 0, 0.0, h)
}

fn square_grid(v: &Values) -> Result<Mesh, String> {
    let (cell, wall, h) = (v.get("cell")?, v.get("wall")?, v.get("height")?);
    let (nx, ny) = (v.count("holes_x")?, v.count("holes_y")?);
    let (xs, ys) = (frame_lines(nx, cell, wall), frame_lines(ny, cell, wall));
    cells(&xs, &ys, |i, j| i % 2 == 0 || j % 2 == 0, 0.0, h)
}

/// A point of a pointy-topped hexagon lattice of circumradius R: (a, b) at (a·√3R/2, b·R/2), a
/// cell's centre at (2i + (j odd), 3j); a corner of a hole (cell index, corner); or a point of a
/// straight edge at a (bottom or top).
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum Hx {
    Lattice(i64, i64),
    Hole(usize, usize),
    Line(i64, bool),
}

/// Corner k of a lattice cell (30° + 60°·k), as lattice steps from its centre.
const HEX_CORNER: [(i64, i64); 6] = [(1, 1), (0, 2), (-1, 1), (-1, -1), (0, -2), (1, -1)];

/// The faces of `nx` × `ny` hexagonal cells round holes of circumradius `hole_r`, cell circumradius
/// `cell_r`; `wrap` lattice columns join up (a cylinder), and `bands` adds straight edges below and
/// above at `bottom` and `top` (lattice v below row 0's lowest corner, and above the last row's).
struct Honeycomb {
    nx: usize,
    ny: usize,
    cell_r: f64,
    hole_r: f64,
    /// v of the lattice's b = 0.
    v0: f64,
    wrap: bool,
    bands: Option<(f64, f64)>,
}

impl Honeycomb {
    fn columns(&self) -> i64 {
        2 * self.nx as i64
    }

    fn key(&self, a: i64, b: i64) -> Hx {
        Hx::Lattice(
            if self.wrap {
                a.rem_euclid(self.columns())
            } else {
                a
            },
            b,
        )
    }

    fn centre(&self, i: usize, j: usize) -> (i64, i64) {
        (2 * i as i64 + (j & 1) as i64, 3 * j as i64)
    }

    fn at(&self, k: Hx) -> P2 {
        let (sx, sy) = (3f64.sqrt() * self.cell_r / 2.0, self.cell_r / 2.0);
        match k {
            Hx::Lattice(a, b) => [a as f64 * sx, self.v0 + b as f64 * sy],
            Hx::Hole(cell, c) => {
                let (i, j) = (cell % self.nx, cell / self.nx);
                let (a, b) = self.centre(i, j);
                let ang = PI / 6.0 + PI / 3.0 * c as f64;
                [
                    a as f64 * sx + self.hole_r * ang.cos(),
                    self.v0 + b as f64 * sy + self.hole_r * ang.sin(),
                ]
            }
            Hx::Line(a, top) => {
                let (bottom, upper) = self.bands.expect("bands asked for");
                [a as f64 * sx, if top { upper } else { bottom }]
            }
        }
    }

    fn faces(&self) -> Vec<Vec<Hx>> {
        let mut faces = Vec::new();
        for j in 0..self.ny {
            for i in 0..self.nx {
                let cell = j * self.nx + i;
                let (a, b) = self.centre(i, j);
                for (k, &c0) in HEX_CORNER.iter().enumerate() {
                    let k1 = (k + 1) % 6;
                    let c1 = HEX_CORNER[k1];
                    faces.push(vec![
                        self.key(a + c0.0, b + c0.1),
                        self.key(a + c1.0, b + c1.1),
                        Hx::Hole(cell, k1),
                        Hx::Hole(cell, k),
                    ]);
                }
            }
        }
        if self.bands.is_some() {
            let cols = self.columns();
            // Below row 0: its corners alternate b = −2 (at even a) and −1 (odd a).
            let low = |a: i64| if a.rem_euclid(2) == 0 { -2 } else { -1 };
            let top_row = self.ny as i64 - 1;
            let shift = top_row & 1;
            let (_, bc) = (0, 3 * top_row);
            let high = |a: i64| {
                if (a - shift).rem_euclid(2) == 0 {
                    bc + 2
                } else {
                    bc + 1
                }
            };
            for a in 0..cols {
                faces.push(vec![
                    self.key_line(a, false),
                    self.key_line(a + 1, false),
                    self.key(a + 1, low(a + 1)),
                    self.key(a, low(a)),
                ]);
                faces.push(vec![
                    self.key(a, high(a)),
                    self.key(a + 1, high(a + 1)),
                    self.key_line(a + 1, true),
                    self.key_line(a, true),
                ]);
            }
        }
        faces
    }

    fn key_line(&self, a: i64, top: bool) -> Hx {
        Hx::Line(
            if self.wrap {
                a.rem_euclid(self.columns())
            } else {
                a
            },
            top,
        )
    }
}

fn hex_grid(v: &Values) -> Result<Mesh, String> {
    let (hole, wall, h) = (v.get("hole")?, v.get("wall")?, v.get("height")?);
    let (nx, ny) = (v.count("holes_x")?, v.count("holes_y")?);
    let hole_r = hole / 2.0;
    // The cell's apothem is the hole's plus half a wall.
    let cell_r = (hole_r * 3f64.sqrt() / 2.0 + wall / 2.0) * 2.0 / 3f64.sqrt();
    let comb = Honeycomb {
        nx,
        ny,
        cell_r,
        hole_r,
        v0: 0.0,
        wrap: false,
        bands: None,
    };
    plate(
        &comb.faces(),
        |k| comb.at(k),
        |p, high| [p[0], p[1], if high { h } else { 0.0 }],
    )
    .finish()
}

fn hex_cylinder(v: &Values) -> Result<Mesh, String> {
    let (radius, height, wall) = (v.get("radius")?, v.get("height")?, v.get("wall")?);
    let (around, bars) = (v.count("around")?, v.get("bars")?);
    if wall >= radius {
        return Err("The wall must be thinner than the radius.".into());
    }
    let circumference = TAU * radius;
    let cell_r = circumference / (3f64.sqrt() * around as f64);
    let hole_r = cell_r - bars / 3f64.sqrt();
    if hole_r <= 0.0 {
        return Err(
            "The bars are too thick for that many holes; use fewer holes or thinner bars.".into(),
        );
    }
    // Rows that fit with a solid band of at least one bar above and below.
    let band = bars.max(wall);
    let rows = ((height - 2.0 * band - 2.0 * cell_r) / (1.5 * cell_r)).floor();
    if rows < 0.0 {
        return Err("The tube is too short for one row of holes; make it taller or use more holes round it.".into());
    }
    let rows = rows as usize + 1;
    let lattice = 1.5 * cell_r * (rows - 1) as f64 + 2.0 * cell_r;
    let bottom_band = (height - lattice) / 2.0;
    let comb = Honeycomb {
        nx: around,
        ny: rows,
        cell_r,
        hole_r,
        v0: bottom_band + cell_r,
        wrap: true,
        bands: Some((0.0, height)),
    };
    let inner = radius - wall;
    plate(
        &comb.faces(),
        |k| comb.at(k),
        |p, outer| {
            let r = if outer { radius } else { inner };
            let a = TAU * p[0] / circumference;
            [r * a.cos(), r * a.sin(), p[1]]
        },
    )
    .finish()
}

// ---------------------------------------------------------------------------------------------
// Pictures

/// The built-in sample picture (used when none is picked): a dark five-pointed star in a dark ring
/// on a light ground that darkens softly towards the middle, 160 × 120.
pub(crate) fn sample_picture() -> Picture {
    let (w, h) = (160usize, 120usize);
    let mut gray = Vec::with_capacity(w * h);
    for row in 0..h {
        for col in 0..w {
            // Centred, y up, in units of the shorter half side.
            let x = (col as f64 + 0.5 - w as f64 / 2.0) / (h as f64 / 2.0);
            let y = (h as f64 / 2.0 - row as f64 - 0.5) / (h as f64 / 2.0);
            let r = x.hypot(y);
            let ring = (0.78..0.92).contains(&r);
            let star = {
                // A five-pointed star of outer radius 0.62 and inner 0.25.
                let a = y.atan2(x) - PI / 2.0;
                let sector = (a.rem_euclid(TAU / 5.0) - TAU / 10.0).abs();
                let (outer, inner) = (0.62, 0.25);
                // The star's edge runs straight from a point to the next notch.
                let t = sector / (TAU / 10.0);
                let edge = 1.0 / ((1.0 - t) / outer + t / inner);
                r < edge
            };
            let ground = 0.95 - 0.35 * (1.0 - r.min(1.4) / 1.4).powi(2);
            gray.push(if ring || star { 0.05 } else { ground as f32 });
        }
    }
    Picture {
        width: w,
        height: h,
        gray,
    }
}

/// The picture's grey at (u, v) in 0..1 across and up (v = 0 at the bottom), bilinear.
fn sample(p: &Picture, u: f64, v: f64) -> f64 {
    let x = (u * p.width as f64 - 0.5).clamp(0.0, (p.width - 1) as f64);
    let y = ((1.0 - v) * p.height as f64 - 0.5).clamp(0.0, (p.height - 1) as f64);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(p.width - 1), (y0 + 1).min(p.height - 1));
    let (fx, fy) = (x - x0 as f64, y - y0 as f64);
    let g = |c: usize, r: usize| p.gray[r * p.width + c] as f64;
    (g(x0, y0) * (1.0 - fx) + g(x1, y0) * fx) * (1.0 - fy)
        + (g(x0, y1) * (1.0 - fx) + g(x1, y1) * fx) * fy
}

/// The size in X and Y a picture takes, and the sample grid over it (at most `detail` along the
/// longer side).
fn picture_grid(v: &Values, p: &Picture) -> Result<(Vec<f64>, Vec<f64>), String> {
    let width = v.get("width")?;
    let depth = if v.toggle("keep_aspect")? {
        width * p.height as f64 / p.width as f64
    } else {
        v.get("depth")?
    };
    let detail = v.count("detail")?;
    let step = width.max(depth) / detail as f64;
    let nx = ((width / step).round() as usize).max(2);
    let ny = ((depth / step).round() as usize).max(2);
    Ok((steps(0.0, width, nx), steps(0.0, depth, ny)))
}

fn picture_or_sample(picked: Option<&Picture>) -> std::borrow::Cow<'_, Picture> {
    match picked {
        Some(p) => std::borrow::Cow::Borrowed(p),
        None => std::borrow::Cow::Owned(sample_picture()),
    }
}

/// How dark (0..1) the picture is at a sample, inverted when asked.
fn darkness(p: &Picture, xs: &[f64], ys: &[f64], i: usize, j: usize, invert: bool) -> f64 {
    let (w, d) = (xs[xs.len() - 1], ys[ys.len() - 1]);
    let g = sample(p, xs[i] / w, ys[j] / d);
    if invert { g } else { 1.0 - g }
}

fn bitmap(v: &Values, picked: Option<&Picture>) -> Result<Mesh, String> {
    let p = picture_or_sample(picked);
    let (xs, ys) = picture_grid(v, &p)?;
    let invert = v.toggle("invert")?;
    let levels = v.count("levels")?;
    let (base, relief) = (v.get("base")?, v.get("relief_height")?);
    let raised = v.choice("relief")? == "raised";
    // Each sample's darkness as a whole level from 0 to levels − 1, so every band edge crosses a
    // grid edge at a fixed share of it (half way between two levels a step apart).
    let (nx, ny) = (xs.len(), ys.len());
    let mut level: Vec<i64> = Vec::with_capacity(nx * ny);
    for j in 0..ny {
        for i in 0..nx {
            let s = darkness(&p, &xs, &ys, i, j, invert) * (levels - 1) as f64;
            level.push(s.round() as i64);
        }
    }
    // Neighbouring samples differ by at most so many levels that the bands between them stay at
    // least 0.01 mm wide: a sharp edge of many levels becomes a steep ramp over a few samples
    // instead of slivers thinner than Repair's tolerance.
    let step = (xs[1] - xs[0]).min(ys[1] - ys[0]);
    let jump = ((step / 0.01).floor() as i64).max(1);
    for pass in 0..2 {
        let order: Vec<usize> = if pass == 0 {
            (0..nx * ny).collect()
        } else {
            (0..nx * ny).rev().collect()
        };
        for g in order {
            let (i, j) = (g % nx, g / nx);
            let before = [
                (pass == 0 && i > 0).then(|| g - 1),
                (pass == 0 && j > 0).then(|| g - nx),
                (pass == 1 && i + 1 < nx).then(|| g + 1),
                (pass == 1 && j + 1 < ny).then(|| g + nx),
            ];
            for n in before.into_iter().flatten() {
                level[g] = level[g].max(level[n] - jump);
            }
        }
    }
    let value: Vec<f64> = level.iter().map(|&l| l as f64).collect();
    let thresholds: Vec<f64> = (0..levels - 1).map(|k| k as f64 + 0.5).collect();
    let heights: Vec<Option<f64>> = (0..levels)
        .map(|k| {
            let f = k as f64 / (levels - 1) as f64;
            Some(if raised {
                base + relief * f
            } else {
                base + relief * (1.0 - f)
            })
        })
        .collect();
    let mesh = terrace(&xs, &ys, &value, &thresholds, &heights, 0.0)?;
    recentre_plain(mesh)
}

fn heightmap(v: &Values, picked: Option<&Picture>) -> Result<Mesh, String> {
    let p = picture_or_sample(picked);
    let (xs, ys) = picture_grid(v, &p)?;
    let invert = v.toggle("invert")?;
    let (base, height) = (v.get("base")?, v.get("height")?);
    let top = |i: usize, j: usize| base + height * darkness(&p, &xs, &ys, i, j, invert);
    heightfield(&xs, &ys, top, |_, _| 0.0).finish()
}

fn qr_code(v: &Values) -> Result<Mesh, String> {
    v.choice("code")?;
    let data = v.text("data")?;
    if data.is_empty() {
        return Err("Type the text the code should hold.".into());
    }
    let ecl = match v.choice("ecc")? {
        "low" => super::Ecc::Low,
        "quartile" => super::Ecc::Quartile,
        "high" => super::Ecc::High,
        _ => super::Ecc::Medium,
    };
    let code = super::encode_qr(&data, ecl)?;
    let quiet = v.count("quiet")?;
    let (size, base, raise) = (v.get("size")?, v.get("base")?, v.get("module_height")?);
    let modules = code.size + 2 * quiet;
    // Four samples across each module, at their middles: the band edges fall on the module
    // boundaries, and corners of dark modules that touch only diagonally stay apart by a hair.
    let per = 4;
    let n = modules * per;
    let cell = size / n as f64;
    let coords: Vec<f64> = (0..n).map(|i| (i as f64 + 0.5) * cell).collect();
    let mut value = Vec::with_capacity(n * n);
    for j in 0..n {
        for i in 0..n {
            // Row 0 of the code is its top: highest y.
            let (mx, my) = (i / per, modules - 1 - j / per);
            let dark = mx >= quiet
                && my >= quiet
                && mx - quiet < code.size
                && my - quiet < code.size
                && code.dark(mx - quiet, my - quiet);
            value.push(if dark { 1.0 } else { 0.0 });
        }
    }
    let mesh = terrace(
        &coords,
        &coords,
        &value,
        &[0.5],
        &[Some(base), Some(base + raise)],
        0.0,
    )?;
    recentre_plain(mesh)
}

fn gear(v: &Values, holes: usize) -> Result<Mesh, String> {
    let g = Gear {
        module: v.get("module")?,
        teeth: v.count("teeth")?,
        pressure_deg: v.get("pressure")?,
        backlash: v.get("backlash")?,
        flank_points: 10,
    };
    gear_mesh(&g, v.get("thickness")?, v.get("bore")?, holes)
}

fn light_gear(v: &Values) -> Result<Mesh, String> {
    gear(v, v.count("holes")?)
}

// ---------------------------------------------------------------------------------------------
// Vase

/// The profile's radius (as a share of the widest) at height share t: a smooth curve through a few
/// control points (Catmull–Rom).
fn vase_profile(kind: &str, t: f64) -> f64 {
    let points: &[(f64, f64)] = match kind {
        "bulb" => &[
            (0.0, 0.55),
            (0.35, 1.0),
            (0.75, 0.42),
            (0.9, 0.36),
            (1.0, 0.42),
        ],
        "tulip" => &[(0.0, 0.45), (0.3, 0.62), (0.6, 0.85), (1.0, 1.0)],
        "cylinder" => &[(0.0, 1.0), (1.0, 1.0)],
        _ => &[
            (0.0, 0.55),
            (0.3, 1.0),
            (0.7, 0.62),
            (0.88, 0.5),
            (1.0, 0.68),
        ],
    };
    let k = points
        .iter()
        .rposition(|p| p.0 <= t)
        .unwrap_or(0)
        .min(points.len() - 2);
    let (p1, p2) = (points[k], points[k + 1]);
    let p0 = if k > 0 { points[k - 1] } else { p1 };
    let p3 = if k + 2 < points.len() {
        points[k + 2]
    } else {
        p2
    };
    let s = ((t - p1.0) / (p2.0 - p1.0)).clamp(0.0, 1.0);
    let (s2, s3) = (s * s, s * s * s);
    0.5 * (2.0 * p1.1
        + (-p0.1 + p2.1) * s
        + (2.0 * p0.1 - 5.0 * p1.1 + 4.0 * p2.1 - p3.1) * s2
        + (-p0.1 + 3.0 * p1.1 - 3.0 * p2.1 + p3.1) * s3)
}

fn vase(v: &Values, control: &mut Control) -> RepairResult<Mesh> {
    let failed = RepairError::Failed;
    let kind = v.choice("profile").map_err(failed)?;
    let (height, width, wall) = (
        v.get("height").map_err(failed)?,
        v.get("width").map_err(failed)?,
        v.get("wall").map_err(failed)?,
    );
    let (lobes, turns, handles, n) = (
        v.count("lobes").map_err(failed)?,
        v.get("turns").map_err(failed)?,
        v.count("handles").map_err(failed)?,
        v.count("segments").map_err(failed)?,
    );
    let rib = if lobes > 0 { 0.06 } else { 0.0 };
    let half = width / 2.0 / (1.0 + rib);
    let outer = |a: f64, z: f64| {
        let t = z / height;
        half * vase_profile(kind, t) * (1.0 + rib * (lobes as f64 * (a - TAU * turns * t)).cos())
    };
    let narrowest = (0..=100)
        .map(|k| vase_profile(kind, k as f64 / 100.0))
        .fold(f64::INFINITY, f64::min)
        * half
        * (1.0 - rib);
    if wall >= narrowest * 0.8 {
        return Err(failed(
            "The wall is too thick for the vase's narrowest part.".into(),
        ));
    }
    let floor = (1.5 * wall).min(height / 4.0);
    let rows = 120;
    let ring = |z: f64, inward: f64| -> Vec<P> {
        (0..n)
            .map(|k| {
                let a = TAU * k as f64 / n as f64;
                let r = outer(a, z) - inward;
                [r * a.cos(), r * a.sin(), z]
            })
            .collect()
    };
    // Up the outside, over the rim, down the inside to the floor.
    let mut rings: Vec<Vec<P>> = (0..=rows)
        .map(|i| ring(height * i as f64 / rows as f64, 0.0))
        .collect();
    let inside_rows = 100;
    for i in (0..=inside_rows).rev() {
        rings.push(ring(
            floor + (height - floor) * i as f64 / inside_rows as f64,
            wall,
        ));
    }
    let mut b = Builder::default();
    b.stack(&rings, Cap::Flat, Cap::Flat);
    let body = b.finish_in_place().map_err(failed)?;
    if handles == 0 {
        return recentre(body);
    }
    // Each handle: a round bar that leaves the wall level, loops out and comes back level lower
    // down, its ends sunk into the wall but not through it.
    let bar = (wall * 1.5).max(2.0).min(height / 12.0);
    let (z_top, z_low) = (0.72 * height, 0.38 * height);
    let mut parts = vec![body];
    for h in 0..handles {
        let a = TAU * (h as f64 + 0.25) / handles as f64;
        let dir = [a.cos(), a.sin()];
        let widest = (0..=50)
            .map(|k| outer(a, z_low + (z_top - z_low) * k as f64 / 50.0))
            .fold(0.0f64, f64::max);
        let reach = widest + bar * 1.5;
        let loop_r = (z_top - z_low) / 2.0;
        let mid = (z_top + z_low) / 2.0;
        // The path in (radius, z): in along the top, round a half circle, back along the bottom.
        let mut path: Vec<(f64, f64)> = Vec::new();
        let sink = 0.5 * wall;
        let segs = 10;
        for k in 0..=segs {
            let s = k as f64 / segs as f64;
            path.push((
                outer(a, z_top) - sink + (reach - outer(a, z_top) + sink) * s,
                z_top,
            ));
        }
        let arc = 32;
        for k in 1..arc {
            let t = PI / 2.0 - PI * k as f64 / arc as f64;
            path.push((reach + loop_r * t.cos(), mid + loop_r * t.sin()));
        }
        for k in 0..=segs {
            let s = k as f64 / segs as f64;
            path.push((reach + (outer(a, z_low) - sink - reach) * s, z_low));
        }
        let points: Vec<P> = path
            .iter()
            .map(|&(r, z)| [r * dir[0], r * dir[1], z])
            .collect();
        parts.push(tube(&points, bar / 2.0, 16, false).map_err(failed)?);
    }
    let joined = union(&parts, control)?;
    recentre(joined)
}

/// A round tube of `radius` along `path` (a closed loop when `closed`, else capped flat at both
/// ends), `around` points round, with frames that turn as little as possible along the path.
pub(crate) fn tube(path: &[P], radius: f64, around: usize, closed: bool) -> Result<Mesh, String> {
    let rings = sweep_rings(path, closed, |k: usize| {
        (0..around)
            .map(|i| {
                let a = TAU * i as f64 / around as f64;
                let _ = k;
                [radius * a.cos(), radius * a.sin()]
            })
            .collect()
    });
    let mut b = Builder::default();
    if closed {
        b.closed_stack(&rings, 0);
    } else {
        b.stack(&rings, Cap::Flat, Cap::Flat);
    }
    b.finish_in_place()
}

/// Rings along a path: ring k is `section(k)` (2D points) laid in the plane across the path at its
/// point k, with rotation-minimising frames (double reflection). A closed path's frames are evened
/// out so the last meets the first.
pub(crate) fn sweep_rings(
    path: &[P],
    closed: bool,
    section: impl Fn(usize) -> Vec<P2>,
) -> Vec<Vec<P>> {
    let n = path.len();
    let sub = |a: P, b: P| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: P, b: P| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let norm = |a: P| {
        let l = dot(a, a).sqrt();
        [a[0] / l, a[1] / l, a[2] / l]
    };
    let cross = |a: P, b: P| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let tangent = |k: usize| {
        let (prev, next) = if closed {
            (path[(k + n - 1) % n], path[(k + 1) % n])
        } else {
            (path[k.saturating_sub(1)], path[(k + 1).min(n - 1)])
        };
        norm(sub(next, prev))
    };
    let t0 = tangent(0);
    let pick = if t0[2].abs() < 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let mut frames: Vec<(P, P)> = Vec::with_capacity(n);
    let mut r = norm(cross(cross(t0, pick), t0));
    frames.push((r, cross(t0, r)));
    for k in 1..n {
        let (x0, x1) = (path[k - 1], path[k]);
        let (ti, tj) = (tangent(k - 1), tangent(k));
        let v1 = sub(x1, x0);
        let c1 = dot(v1, v1);
        let rl = sub(r, scale(v1, 2.0 * dot(v1, r) / c1));
        let tl = sub(ti, scale(v1, 2.0 * dot(v1, ti) / c1));
        let v2 = sub(tj, tl);
        let c2 = dot(v2, v2);
        r = if c2 > 0.0 {
            sub(rl, scale(v2, 2.0 * dot(v2, rl) / c2))
        } else {
            rl
        };
        r = norm(r);
        frames.push((r, cross(tj, r)));
    }
    if closed {
        // The angle the frames have turned by once round, spread over the loop.
        let (r_end, _) = frames[n - 1];
        let t = tangent(0);
        let (r0, s0) = frames[0];
        let twist = dot(r_end, s0).atan2(dot(r_end, r0));
        for (k, f) in frames.iter_mut().enumerate() {
            let a = -twist * k as f64 / n as f64;
            let tk = tangent(k);
            let (r, s) = *f;
            let nr = norm(add(scale(r, a.cos()), scale(s, a.sin())));
            *f = (nr, cross(tk, nr));
            let _ = t;
        }
    }
    (0..n)
        .map(|k| {
            let (r, s) = frames[k];
            section(k)
                .into_iter()
                .map(|q| add(path[k], add(scale(r, q[0]), scale(s, q[1]))))
                .collect()
        })
        .collect()
}

fn scale(a: P, s: f64) -> P {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn add(a: P, b: P) -> P {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

// ---------------------------------------------------------------------------------------------
// Picture sweep

/// The outlines of the dark area of a sampled picture (`dark[j * nx + i]`), as loops of points in
/// sample units: the dark area on the left of each, so outer outlines run anticlockwise and holes
/// clockwise. The samples outside the grid count as light, so every loop closes. Where only two
/// diagonal samples are dark, they stay apart.
pub(crate) fn outlines(dark: &[bool], nx: usize, ny: usize) -> Vec<Vec<P2>> {
    let at = |i: isize, j: isize| {
        i >= 0
            && j >= 0
            && (i as usize) < nx
            && (j as usize) < ny
            && dark[j as usize * nx + i as usize]
    };
    // Edge points by the sample pair they lie between: (i, j, horizontal).
    type E = (isize, isize, bool);
    let mut next: HashMap<E, E> = HashMap::new();
    for j in -1..ny as isize {
        for i in -1..nx as isize {
            let (bl, br, tr, tl) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            let b: E = (i, j, true);
            let t: E = (i, j + 1, true);
            let l: E = (i, j, false);
            let r: E = (i + 1, j, false);
            let case = u8::from(bl) | u8::from(br) << 1 | u8::from(tr) << 2 | u8::from(tl) << 3;
            let segments: &[(E, E)] = match case {
                1 => &[(b, l)],
                2 => &[(r, b)],
                3 => &[(r, l)],
                4 => &[(t, r)],
                5 => &[(b, l), (t, r)],
                6 => &[(t, b)],
                7 => &[(t, l)],
                8 => &[(l, t)],
                9 => &[(b, t)],
                10 => &[(r, b), (l, t)],
                11 => &[(r, t)],
                12 => &[(l, r)],
                13 => &[(b, r)],
                14 => &[(l, b)],
                _ => &[],
            };
            for &(from, to) in segments {
                next.insert(from, to);
            }
        }
    }
    let place = |e: E| -> P2 {
        let (i, j, horizontal) = e;
        if horizontal {
            [i as f64 + 0.5, j as f64]
        } else {
            [i as f64, j as f64 + 0.5]
        }
    };
    let mut loops = Vec::new();
    let mut used: HashSet<E> = HashSet::new();
    let mut starts: Vec<E> = next.keys().copied().collect();
    starts.sort();
    for start in starts {
        if used.contains(&start) {
            continue;
        }
        let mut lp = Vec::new();
        let mut e = start;
        loop {
            used.insert(e);
            lp.push(place(e));
            e = next[&e];
            if e == start {
                break;
            }
        }
        // Straight runs keep only their ends.
        let n = lp.len();
        let kept: Vec<P2> = (0..n)
            .filter(|&k| {
                let (a, b, c) = (lp[(k + n - 1) % n], lp[k], lp[(k + 1) % n]);
                (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]) != 0.0
            })
            .map(|k| lp[k])
            .collect();
        if kept.len() >= 3 {
            loops.push(kept);
        }
    }
    loops
}

/// Outer outlines with the holes inside each.
pub(crate) fn polygons_of(loops: Vec<Vec<P2>>) -> Vec<Polygon> {
    let (outer, holes): (Vec<_>, Vec<_>) = loops.into_iter().partition(|l| area2(l) > 0.0);
    let inside = |p: P2, lp: &[P2]| {
        let mut c = false;
        let n = lp.len();
        for k in 0..n {
            let (a, b) = (lp[k], lp[(k + 1) % n]);
            if (a[1] > p[1]) != (b[1] > p[1])
                && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0])
            {
                c = !c;
            }
        }
        c
    };
    let mut polys: Vec<(Vec<P2>, Vec<Vec<P2>>)> =
        outer.into_iter().map(|o| (o, Vec::new())).collect();
    for h in holes {
        // The smallest outline round the hole.
        if let Some(k) = (0..polys.len())
            .filter(|&k| inside(h[0], &polys[k].0))
            .min_by(|&a, &b| area2(&polys[a].0).total_cmp(&area2(&polys[b].0)))
        {
            polys[k].1.push(h);
        }
    }
    polys.into_iter().map(|(o, h)| Polygon::new(o, h)).collect()
}

fn picture_sweep(v: &Values, picked: Option<&Picture>) -> Result<Mesh, String> {
    let p = picture_or_sample(picked);
    let invert = v.toggle("invert")?;
    let size = v.get("size")?;
    let (length, spin, steps_n) = (
        v.get("length")?,
        v.get("spin")?.to_radians(),
        v.count("steps")?,
    );
    // The outline from a grid of at most 120 samples along the longer side.
    let detail = 120.0;
    let step = p.width.max(p.height) as f64 / detail;
    let (nx, ny) = (
        ((p.width as f64 / step).round() as usize).max(2),
        ((p.height as f64 / step).round() as usize).max(2),
    );
    let mut dark = Vec::with_capacity(nx * ny);
    for j in 0..ny {
        for i in 0..nx {
            let g = sample(
                p.as_ref(),
                (i as f64 + 0.5) / nx as f64,
                (j as f64 + 0.5) / ny as f64,
            );
            dark.push(if invert { g > 0.5 } else { g < 0.5 });
        }
    }
    let polys = polygons_of(outlines(&dark, nx, ny));
    if polys.is_empty() {
        return Err("The picture has nothing dark to sweep; try Invert colours.".into());
    }
    // Scaled so the longer side of the outlines' box is `size`, centred on the origin.
    let pts: Vec<P2> = polys
        .iter()
        .flat_map(|p| p.loops().flatten().copied().collect::<Vec<_>>())
        .collect();
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for q in &pts {
        for k in 0..2 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    let s = size / (hi[0] - lo[0]).max(hi[1] - lo[1]);
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let polys: Vec<Polygon> = polys
        .iter()
        .map(|poly| {
            let f = |lp: &Vec<P2>| {
                lp.iter()
                    .map(|q| [(q[0] - c[0]) * s, (q[1] - c[1]) * s])
                    .collect::<Vec<P2>>()
            };
            Polygon::new(f(&poly.outer), poly.holes.iter().map(f).collect())
        })
        .collect();
    let arc = v.choice("path")? == "arc";
    let reach = s * (hi[0] - lo[0]) / 2.0;
    // A quarter circle whose length is the path length; it must bend wider than the outline.
    let bend = length / (PI / 2.0);
    if arc && bend <= reach * 1.05 {
        return Err(format!(
            "The quarter circle is too tight for the picture; make the path longer than {:.1} mm.",
            reach * 1.05 * PI / 2.0
        ));
    }
    let place = |q: P2, k: usize| -> P {
        let f = k as f64 / steps_n as f64;
        let a = spin * f;
        let (x, y) = (
            q[0] * a.cos() - q[1] * a.sin(),
            q[0] * a.sin() + q[1] * a.cos(),
        );
        if arc {
            // Slices stand across a quarter circle round the Y axis at distance `bend`.
            let b = PI / 2.0 * f;
            let r = bend + x;
            [r * b.cos(), y, r * b.sin()]
        } else {
            [x, y, length * f]
        }
    };
    let mut b = Builder::default();
    for poly in &polys {
        let loops: Vec<&Vec<P2>> = poly.loops().collect();
        let mut slices: Vec<Vec<Vec<u32>>> = Vec::new();
        for k in 0..=steps_n {
            slices.push(
                loops
                    .iter()
                    .map(|lp| lp.iter().map(|&q| b.point(place(q, k))).collect())
                    .collect(),
            );
        }
        for k in 0..steps_n {
            for (lo_ids, hi_ids) in slices[k].iter().zip(&slices[k + 1]) {
                b.strip(lo_ids, hi_ids, true);
            }
        }
        let first: Vec<u32> = slices[0].iter().flatten().copied().collect();
        let last: Vec<u32> = slices[steps_n].iter().flatten().copied().collect();
        for [x, y, z] in poly.triangulate()? {
            b.triangle(first[x], first[z], first[y]);
            b.triangle(last[x], last[y], last[z]);
        }
    }
    b.finish()
}

// ---------------------------------------------------------------------------------------------
// Bracelet and brick

fn bracelet(v: &Values) -> Result<Mesh, String> {
    let style = v.choice("style")?;
    let wrist = v.get("wrist")?;
    let fit = match v.choice("fit")? {
        "close" => 1.0,
        "loose" => 1.12,
        _ => 1.05,
    };
    let (width, thick) = (v.get("band_width")?, v.get("thickness")?);
    let (around, across) = (v.count("around")?, v.count("across")?);
    let r_in = wrist * fit / TAU;
    if thick >= r_in || width / 2.0 >= r_in {
        return Err("The band is too big for that wrist.".into());
    }
    // The band's cross-section: a rounded slot (thickness across, band width up), `across` points
    // round, anticlockwise in (radial, up).
    let section = |w: f64, turn: f64| -> Vec<P2> {
        let r = thick / 2.0;
        let straight = (w - thick).max(0.0) / 2.0;
        (0..across)
            .map(|k| {
                // A circle's upper half lifted and its lower half lowered: a stadium whose straight
                // sides run up the band.
                let a = TAU * (k as f64 + 0.5) / across as f64;
                let (sx, sy) = (r * a.cos(), r * a.sin());
                let lift = if sy >= 0.0 { straight } else { -straight };
                let (x, y) = (sx, sy + lift);
                [
                    x * turn.cos() - y * turn.sin(),
                    x * turn.sin() + y * turn.cos(),
                ]
            })
            .collect()
    };
    let centre = r_in + thick / 2.0;
    let half_twists = 6.0;
    let rings: Vec<Vec<P>> = (0..around)
        .map(|k| {
            let a = TAU * k as f64 / around as f64;
            let (w, z, turn) = match style {
                "wave" => (
                    width * (1.0 + 0.25 * (6.0 * a).sin()),
                    0.2 * width * (6.0 * a).cos(),
                    0.0,
                ),
                "twisted" => (width, 0.0, half_twists * a / 2.0),
                _ => (width, 0.0, 0.0),
            };
            section(w, turn)
                .into_iter()
                .map(|q| {
                    [
                        (centre + q[0]) * a.cos(),
                        (centre + q[0]) * a.sin(),
                        z + q[1],
                    ]
                })
                .collect()
        })
        .collect();
    let mut b = Builder::default();
    b.closed_stack(&rings, 0);
    b.finish()
}

fn brick(v: &Values, control: &mut Control) -> RepairResult<Mesh> {
    let failed = RepairError::Failed;
    let (nx, ny) = (
        v.count("studs_x").map_err(failed)?,
        v.count("studs_y").map_err(failed)?,
    );
    let height = if v.choice("kind").map_err(failed)? == "plate" {
        3.2
    } else {
        9.6
    };
    let hollow = v.toggle("hollow").map_err(failed)?;
    let pitch = 8.0;
    let (l, w) = (pitch * nx as f64 - 0.2, pitch * ny as f64 - 0.2);
    let block = |x0: f64, y0: f64, x1: f64, y1: f64, z0: f64, z1: f64| {
        crate::mesh::make_box(
            [x0 as f32, y0 as f32, z0 as f32],
            [x1 as f32, y1 as f32, z1 as f32],
        )
    };
    let disc = |cx: f64, cy: f64, r: f64, hole: f64, z0: f64, z1: f64| -> Result<Mesh, String> {
        let holes = if hole > 0.0 {
            vec![circle_2d([cx, cy], hole, 48, 0.0)]
        } else {
            Vec::new()
        };
        let mut b = Builder::default();
        extrude(
            &mut b,
            &[Polygon::new(circle_2d([cx, cy], r, 48, 0.0), holes)],
            z0,
            z1,
        )?;
        b.finish_in_place()
    };
    let overlap = 0.05;
    let mut parts = Vec::new();
    let top = 1.0f64.min(height / 3.0);
    let wall = 1.5f64.min(l.min(w) / 3.0);
    if hollow {
        parts.push(block(0.0, 0.0, l, w, height - top, height));
        parts.push(block(0.0, 0.0, wall, w, 0.0, height - top + overlap));
        parts.push(block(l - wall, 0.0, l, w, 0.0, height - top + overlap));
        parts.push(block(
            wall - overlap,
            0.0,
            l - wall + overlap,
            wall,
            0.0,
            height - top + overlap,
        ));
        parts.push(block(
            wall - overlap,
            w - wall,
            l - wall + overlap,
            w,
            0.0,
            height - top + overlap,
        ));
        // Tubes between the studs grip the studs of a brick below.
        if nx >= 2 && ny >= 2 {
            for i in 1..nx {
                for j in 1..ny {
                    let (cx, cy) = (i as f64 * pitch - 0.1, j as f64 * pitch - 0.1);
                    parts.push(
                        disc(cx, cy, 6.51 / 2.0, 4.8 / 2.0, 0.0, height - top + overlap)
                            .map_err(failed)?,
                    );
                }
            }
        }
    } else {
        parts.push(block(0.0, 0.0, l, w, 0.0, height));
    }
    for i in 0..nx {
        for j in 0..ny {
            let (cx, cy) = (
                (i as f64 + 0.5) * pitch - 0.1,
                (j as f64 + 0.5) * pitch - 0.1,
            );
            parts.push(
                disc(cx, cy, 4.8 / 2.0, 0.0, height - overlap, height + 1.7).map_err(failed)?,
            );
        }
    }
    let joined = union(&parts, control)?;
    recentre(joined)
}

// ---------------------------------------------------------------------------------------------
// Base plate

/// Squared distance from each sample to the nearest marked one, in samples² (Felzenszwalb and
/// Huttenlocher's exact transform, one row then one column at a time).
fn distance2(marked: &[bool], nx: usize, ny: usize) -> Vec<f64> {
    // Far, yet small enough that adding a squared index keeps its last digits.
    const FAR: f64 = 1e12;
    let pass = |f: &[f64]| -> Vec<f64> {
        let n = f.len();
        let mut d = vec![0.0; n];
        let mut v = vec![0usize; n];
        let mut z = vec![0.0f64; n + 1];
        let mut k = 0usize;
        z[0] = f64::NEG_INFINITY;
        z[1] = f64::INFINITY;
        let meet = |q: usize, p: usize| {
            ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * q as f64 - 2.0 * p as f64)
        };
        for q in 1..n {
            let mut s = meet(q, v[k]);
            while s <= z[k] {
                k -= 1;
                s = meet(q, v[k]);
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f64::INFINITY;
        }
        k = 0;
        for (q, out) in d.iter_mut().enumerate() {
            while z[k + 1] < q as f64 {
                k += 1;
            }
            let p = v[k];
            *out = (q as f64 - p as f64).powi(2) + f[p];
        }
        d
    };
    let mut grid: Vec<f64> = marked.iter().map(|&m| if m { 0.0 } else { FAR }).collect();
    for j in 0..ny {
        let row = pass(&grid[j * nx..(j + 1) * nx]);
        grid[j * nx..(j + 1) * nx].copy_from_slice(&row);
    }
    for i in 0..nx {
        let col: Vec<f64> = (0..ny).map(|j| grid[j * nx + i]).collect();
        let out = pass(&col);
        for j in 0..ny {
            grid[j * nx + i] = out[j];
        }
    }
    grid
}

fn base_plate(v: &Values, sources: &Sources, control: &mut Control) -> RepairResult<Mesh> {
    let failed = RepairError::Failed;
    let sample_part;
    let parts: &[Mesh] = if sources.parts.is_empty() {
        // The tile picture: a sample part, an L-shaped block.
        let mut b = Builder::default();
        b.append(&crate::mesh::make_box([0.0, 0.0, 0.0], [30.0, 10.0, 10.0]));
        b.append(&crate::mesh::make_box([0.0, 10.0, 0.0], [10.0, 25.0, 10.0]));
        sample_part = vec![b.raw().map_err(failed)?];
        &sample_part
    } else {
        &sources.parts
    };
    let kind = v.choice("type").map_err(failed)?;
    let thickness = v.get("thickness").map_err(failed)?;
    let (cell, wall) = (
        v.get("cell").map_err(failed)?,
        v.get("wall").map_err(failed)?,
    );
    let offset = v.get("offset").map_err(failed)?;
    let outer_only = v.toggle("outer_only").map_err(failed)?;
    let invert = v.toggle("invert").map_err(failed)?;
    let outer_offset = if invert {
        v.get("outer_offset").map_err(failed)?
    } else {
        0.0
    };
    let lower = if v.toggle("use_lower").map_err(failed)? {
        Some(v.get("lower").map_err(failed)?)
    } else {
        None
    };
    let upper = if v.toggle("use_upper").map_err(failed)? {
        Some(v.get("upper").map_err(failed)?)
    } else {
        None
    };
    // The part's box and its lowest point, where the plate's top goes.
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for m in parts {
        if let Some(b) = bounds(m) {
            for k in 0..3 {
                lo[k] = lo[k].min(b[0][k] as f64);
                hi[k] = hi[k].max(b[1][k] as f64);
            }
        }
    }
    if !lo[0].is_finite() {
        return Err(failed("The selected part has no triangles.".into()));
    }
    // Samples fine enough for the walls, but not more than about 4 million.
    let pad = offset + outer_offset + 2.0 * cell.max(wall) + 1.0;
    let (x0, y0) = (lo[0] - pad, lo[1] - pad);
    let (sx, sy) = (hi[0] - lo[0] + 2.0 * pad, hi[1] - lo[1] + 2.0 * pad);
    let mut step = (wall.min(cell) / 3.0).clamp(0.05, 2.0);
    while (sx / step) * (sy / step) > 4.0e6 {
        step *= 1.25;
    }
    let (nx, ny) = (
        (sx / step).ceil() as usize + 1,
        (sy / step).ceil() as usize + 1,
    );
    let px = |i: usize| x0 + i as f64 * step;
    let py = |j: usize| y0 + j as f64 * step;
    // The shadow: every sample under a triangle (or the piece of it between the limits).
    let mut shadow = vec![false; nx * ny];
    for m in parts {
        for (n, t) in m.indices.as_chunks::<3>().0.iter().enumerate() {
            if n % 4096 == 0 {
                control.check()?;
            }
            let poly: Vec<P> = t.iter().map(|&i| m.point(i)).collect();
            let poly = clip_z(poly, lower, upper);
            if poly.len() < 3 {
                continue;
            }
            let (mut a, mut b) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
            for q in &poly {
                for k in 0..2 {
                    a[k] = a[k].min(q[k]);
                    b[k] = b[k].max(q[k]);
                }
            }
            let i0 = ((a[0] - x0) / step).floor().max(0.0) as usize;
            let i1 = (((b[0] - x0) / step).ceil() as usize).min(nx - 1);
            let j0 = ((a[1] - y0) / step).floor().max(0.0) as usize;
            let j1 = (((b[1] - y0) / step).ceil() as usize).min(ny - 1);
            // The projected piece is convex; a sample is under it when it is on one side of
            // every edge (either side, as the piece may face down). Edge-on pieces cover nothing.
            let k = poly.len();
            let turn: f64 = (0..k)
                .map(|e| {
                    let (p, q) = (poly[e], poly[(e + 1) % k]);
                    p[0] * q[1] - q[0] * p[1]
                })
                .sum();
            if turn == 0.0 {
                continue;
            }
            for j in j0..=j1 {
                for i in i0..=i1 {
                    let (x, y) = (px(i), py(j));
                    let under = (0..k).all(|e| {
                        let (p, q) = (poly[e], poly[(e + 1) % k]);
                        ((q[0] - p[0]) * (y - p[1]) - (q[1] - p[1]) * (x - p[0])) * turn >= 0.0
                    });
                    if under {
                        shadow[j * nx + i] = true;
                    }
                }
            }
        }
    }
    if !shadow.iter().any(|&s| s) {
        return Err(failed(
            "The part casts no shadow between the limits.".into(),
        ));
    }
    if outer_only {
        shadow = fill_enclosed(&shadow, nx, ny);
    }
    let to_shadow = distance2(&shadow, nx, ny);
    let near = |g: usize, d: f64| to_shadow[g] * step * step <= d * d;
    let mut region: Vec<bool> = (0..nx * ny).map(|g| near(g, offset)).collect();
    if invert {
        // The box of the (offset) shadow padded by the outer offset, without the shadow.
        let (mut a, mut b) = ([usize::MAX; 2], [0usize; 2]);
        for j in 0..ny {
            for i in 0..nx {
                if region[j * nx + i] {
                    a = [a[0].min(i), a[1].min(j)];
                    b = [b[0].max(i), b[1].max(j)];
                }
            }
        }
        let grow = (outer_offset / step).round() as usize;
        let inside = |i: usize, j: usize| {
            i + grow >= a[0] && i <= b[0] + grow && j + grow >= a[1] && j <= b[1] + grow
        };
        region = (0..nx * ny)
            .map(|g| inside(g % nx, g / nx) && !shadow[g])
            .collect();
    }
    if kind != "solid" {
        // Holes everywhere but on the walls of the cell pattern and on a rim of one wall all round.
        let outside: Vec<bool> = region.iter().map(|&r| !r).collect();
        let to_outside = distance2(&outside, nx, ny);
        let pitch = cell + wall;
        for g in 0..nx * ny {
            if !region[g] || to_outside[g] * step * step <= wall * wall {
                continue;
            }
            let (x, y) = (px(g % nx), py(g / nx));
            let on_wall = if kind == "square" {
                let (u, w) = ((x - x0).rem_euclid(pitch), (y - y0).rem_euclid(pitch));
                u < wall || w < wall
            } else {
                hex_wall(x - x0, y - y0, cell, wall)
            };
            region[g] = on_wall;
        }
    }
    let value: Vec<f64> = region.iter().map(|&r| if r { 1.0 } else { 0.0 }).collect();
    let xs: Vec<f64> = (0..nx).map(px).collect();
    let ys: Vec<f64> = (0..ny).map(py).collect();
    let top = lo[2];
    control.check()?;
    let mesh = terrace(
        &xs,
        &ys,
        &value,
        &[0.5],
        &[None, Some(top)],
        top - thickness,
    )
    .map_err(failed)?;
    if sources.parts.is_empty() {
        // The tile picture shows the plate alone.
        return recentre(mesh);
    }
    Ok(mesh)
}

/// True where (x, y) lies on the walls of a honeycomb whose holes are `cell` across their flats,
/// with walls `wall` thick.
fn hex_wall(x: f64, y: f64, cell: f64, wall: f64) -> bool {
    // Pointy-topped cells of apothem (cell + wall) / 2.
    let apothem = (cell + wall) / 2.0;
    let r = apothem * 2.0 / 3f64.sqrt();
    // Axial coordinates, rounded to the nearest cell.
    let q = (3f64.sqrt() / 3.0 * x - y / 3.0) / r;
    let s = (2.0 / 3.0 * y) / r;
    let (mut rq, mut rs) = (q.round(), s.round());
    let rr = (-q - s).round();
    let (dq, ds, dr) = ((rq - q).abs(), (rs - s).abs(), (rr - (-q - s)).abs());
    if dq > ds && dq > dr {
        rq = -rs - rr;
    } else if ds > dr {
        rs = -rq - rr;
    }
    let cx = r * 3f64.sqrt() * (rq + rs / 2.0);
    let cy = r * 1.5 * rs;
    let (dx, dy) = (x - cx, y - cy);
    // Distance to the centre across the nearest flat (flats face 0°, 60°, 120° …).
    let across = [0.0f64, PI / 3.0, 2.0 * PI / 3.0]
        .iter()
        .map(|a| (dx * a.cos() + dy * a.sin()).abs())
        .fold(0.0, f64::max);
    across > cell / 2.0
}

/// The shadow with every unshadowed area it surrounds filled in: what the outside cannot reach.
fn fill_enclosed(shadow: &[bool], nx: usize, ny: usize) -> Vec<bool> {
    let mut outside = vec![false; nx * ny];
    let mut stack: Vec<usize> = Vec::new();
    for i in 0..nx {
        stack.push(i);
        stack.push((ny - 1) * nx + i);
    }
    for j in 0..ny {
        stack.push(j * nx);
        stack.push(j * nx + nx - 1);
    }
    while let Some(g) = stack.pop() {
        if outside[g] || shadow[g] {
            continue;
        }
        outside[g] = true;
        let (i, j) = (g % nx, g / nx);
        if i > 0 {
            stack.push(g - 1);
        }
        if i + 1 < nx {
            stack.push(g + 1);
        }
        if j > 0 {
            stack.push(g - nx);
        }
        if j + 1 < ny {
            stack.push(g + nx);
        }
    }
    outside.iter().map(|&o| !o).collect()
}

/// The part of a convex polygon between the limits.
fn clip_z(poly: Vec<P>, lower: Option<f64>, upper: Option<f64>) -> Vec<P> {
    let mut out = poly;
    for (limit, keep_above) in [(lower, true), (upper, false)] {
        let Some(z) = limit else { continue };
        let inside = |p: &P| if keep_above { p[2] >= z } else { p[2] <= z };
        let n = out.len();
        let mut next = Vec::with_capacity(n + 2);
        for k in 0..n {
            let (a, b) = (out[k], out[(k + 1) % n]);
            if inside(&a) {
                next.push(a);
            }
            if inside(&a) != inside(&b) {
                let t = (z - a[2]) / (b[2] - a[2]);
                next.push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]), z]);
            }
        }
        out = next;
        if out.is_empty() {
            break;
        }
    }
    out
}
