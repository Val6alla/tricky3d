//! The scene graph VRML and X3D share: nodes with named fields, read by `vrml` (the classic text
//! encoding, also X3D's) and `x3d` (the XML encoding), and turned into objects here.
//!
//! What counts: Transform (translation, rotation, scale, scale orientation, centre) and every other
//! node that holds children (Group, Anchor, Collision, the CAD nodes…; a Switch only its chosen
//! child, an LOD only its first level); in a Shape, its geometry and its Material's diffuse colour.
//! Geometry: IndexedFaceSet (polygons fanned, or ear-clipped when `convex` is false), the triangle
//! sets, strips and fans, and Box, Sphere, Cylinder and Cone. Colour nodes give each triangle a
//! colour (the mean of its corners' when colours are per vertex). ElevationGrid, Extrusion, text and
//! lines are skipped with a warning, and so are Inline files (they are never opened).
//!
//! An object is the shapes under one name: a Shape's own DEF name, else that of the nearest named
//! node round it. Shapes with no name anywhere are an object each.

use std::collections::HashMap;
use std::rc::Rc;

use crate::mesh::Matrix;

pub(crate) struct Node {
    pub kind: String,
    /// The DEF name.
    pub name: Option<String>,
    pub fields: Vec<(String, Field)>,
}

pub(crate) enum Field {
    Numbers(Vec<f64>),
    Bool(bool),
    Text(Vec<String>),
    Nodes(Vec<Rc<Node>>),
}

impl Node {
    pub fn new(kind: &str) -> Node {
        Node {
            kind: kind.to_string(),
            name: None,
            fields: Vec::new(),
        }
    }

    fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|(n, _)| n == name).map(|(_, f)| f)
    }

    pub fn numbers(&self, name: &str) -> Option<&[f64]> {
        match self.field(name)? {
            Field::Numbers(v) => Some(v),
            _ => None,
        }
    }

    fn number(&self, name: &str, default: f64) -> f64 {
        self.numbers(name)
            .and_then(|v| v.first().copied())
            .unwrap_or(default)
    }

    fn vector<const N: usize>(&self, name: &str, default: [f64; N]) -> [f64; N] {
        match self.numbers(name) {
            Some(v) if v.len() >= N => std::array::from_fn(|i| v[i]),
            _ => default,
        }
    }

    pub fn flag(&self, name: &str, default: bool) -> bool {
        match self.field(name) {
            Some(Field::Bool(b)) => *b,
            Some(Field::Text(t)) => t
                .first()
                .map_or(default, |s| s.eq_ignore_ascii_case("true")),
            _ => default,
        }
    }

    /// Every node held in any field, in order.
    pub fn children(&self) -> impl Iterator<Item = &Rc<Node>> {
        self.fields.iter().flat_map(|(_, f)| match f {
            Field::Nodes(n) => n.as_slice(),
            _ => &[],
        })
    }

    /// The nodes held in field `name` (VRML `children`, X3D child elements whose field is not
    /// written out are filed under the child's default field by the reader).
    fn nodes(&self, name: &str) -> &[Rc<Node>] {
        match self.field(name) {
            Some(Field::Nodes(n)) => n,
            _ => &[],
        }
    }

    /// The first child node of one of these kinds, in any field.
    fn child(&self, kinds: &[&str]) -> Option<&Rc<Node>> {
        self.children().find(|n| kinds.contains(&n.kind.as_str()))
    }
}

/// The grouping nodes whose `children`… and so on hold the scene; anything else that is not a
/// Shape is looked into too, so unknown grouping nodes still give their shapes.
const GEOMETRY: &[&str] = &[
    "IndexedFaceSet",
    "IndexedTriangleSet",
    "TriangleSet",
    "IndexedTriangleFanSet",
    "IndexedTriangleStripSet",
    "TriangleFanSet",
    "TriangleStripSet",
    "Box",
    "Sphere",
    "Cylinder",
    "Cone",
    "ElevationGrid",
    "Extrusion",
    "Text",
    "IndexedLineSet",
    "LineSet",
    "PointSet",
];

/// Nodes never looked into for shapes.
const LEAVES: &[&str] = &[
    "Appearance",
    "Material",
    "TwoSidedMaterial",
    "ImageTexture",
    "PixelTexture",
    "MovieTexture",
    "TextureTransform",
    "Coordinate",
    "CoordinateDouble",
    "Color",
    "ColorRGBA",
    "Normal",
    "TextureCoordinate",
    "Script",
    "Viewpoint",
    "NavigationInfo",
    "Background",
    "WorldInfo",
    "DirectionalLight",
    "PointLight",
    "SpotLight",
    "Fog",
    "Sound",
    "AudioClip",
    "TimeSensor",
    "TouchSensor",
    "ProximitySensor",
];

/// One object: world-space triangles (not welded), its part colour, and a colour per triangle when
/// the file gives more than one.
pub(crate) struct SceneObject {
    pub name: Option<String>,
    pub positions: Vec<f32>,
    pub indices: Vec<u32>,
    pub color: Option<[u8; 3]>,
    pub colors: Option<Vec<u8>>,
}

/// One shape's triangles in world space, with its colours.
struct Piece {
    scope: Option<String>,
    named: bool,
    positions: Vec<f64>,
    triangles: Vec<[u32; 3]>,
    /// Per triangle, when the geometry has colours.
    colors: Option<Vec<[u8; 3]>>,
    material: Option<[u8; 3]>,
}

#[derive(Default)]
struct Skipped {
    kinds: Vec<String>,
    inline: usize,
}

/// The objects of a scene, every length times `scale` (mm per file unit).
pub(crate) fn collect(
    roots: &[Rc<Node>],
    scale: f64,
    warnings: &mut Vec<String>,
) -> Vec<SceneObject> {
    let mut pieces = Vec::new();
    let mut skipped = Skipped::default();
    let start = scaling(scale);
    for root in roots {
        walk(root, &start, None, &mut pieces, &mut skipped, 0);
    }
    if !skipped.kinds.is_empty() {
        warnings.push(format!(
            "Some shapes were left out: tricky3D does not read {} nodes.",
            list(&skipped.kinds)
        ));
    }
    if skipped.inline > 0 {
        warnings.push(format!(
            "{} other {} the file refers to (Inline) {} not opened.",
            skipped.inline,
            if skipped.inline == 1 { "file" } else { "files" },
            if skipped.inline == 1 { "was" } else { "were" }
        ));
    }
    group(pieces)
}

fn list(kinds: &[String]) -> String {
    match kinds {
        [one] => one.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
        [] => String::new(),
    }
}

/// Deep enough for any real file; a cycle through USE cannot recurse past it.
const MAX_DEPTH: usize = 256;

fn walk(
    node: &Rc<Node>,
    m: &Matrix,
    scope: Option<&str>,
    out: &mut Vec<Piece>,
    skipped: &mut Skipped,
    depth: usize,
) {
    if depth > MAX_DEPTH || LEAVES.contains(&node.kind.as_str()) {
        return;
    }
    let scope = node.name.as_deref().or(scope);
    match node.kind.as_str() {
        "Shape" => shape(node, m, scope, out, skipped),
        "Inline" => skipped.inline += 1,
        "Transform" => {
            let local = crate::mesh::multiply(m, &transform(node));
            for c in node.children() {
                walk(c, &local, scope, out, skipped, depth + 1);
            }
        }
        "Switch" => {
            let choice = node.number("whichChoice", -1.0);
            let list = if node.nodes("choice").is_empty() {
                node.nodes("children")
            } else {
                node.nodes("choice")
            };
            if choice >= 0.0
                && let Some(c) = list.get(choice as usize)
            {
                walk(c, m, scope, out, skipped, depth + 1);
            }
        }
        "LOD" => {
            let list = if node.nodes("level").is_empty() {
                node.nodes("children")
            } else {
                node.nodes("level")
            };
            if let Some(c) = list.first() {
                walk(c, m, scope, out, skipped, depth + 1);
            }
        }
        kind if GEOMETRY.contains(&kind) => {}
        _ => {
            for c in node.children() {
                walk(c, m, scope, out, skipped, depth + 1);
            }
        }
    }
}

fn shape(
    node: &Node,
    m: &Matrix,
    scope: Option<&str>,
    out: &mut Vec<Piece>,
    skipped: &mut Skipped,
) {
    let Some(geometry) = node.child(GEOMETRY) else {
        return;
    };
    let material = node
        .child(&["Appearance"])
        .and_then(|a| a.child(&["Material", "TwoSidedMaterial"]))
        .map(|mat| to_rgb(mat.vector("diffuseColor", [0.8, 0.8, 0.8])));
    let Some((local, triangles, colors)) = triangulate(geometry) else {
        let kind = geometry.kind.clone();
        if !skipped.kinds.contains(&kind) {
            skipped.kinds.push(kind);
        }
        return;
    };
    if triangles.is_empty() {
        return;
    }
    let mut positions = Vec::with_capacity(local.len());
    for p in local.as_chunks::<3>().0 {
        positions.extend(crate::mesh::transform_point(m, *p));
    }
    // A mirroring transform turns the triangles round, so they keep facing out.
    let triangles = if crate::mesh::linear_determinant(m) < 0.0 {
        triangles.into_iter().map(|[a, b, c]| [a, c, b]).collect()
    } else {
        triangles
    };
    out.push(Piece {
        scope: scope.map(str::to_string),
        named: scope.is_some(),
        positions,
        triangles,
        colors,
        material,
    });
}

fn to_rgb(c: [f64; 3]) -> [u8; 3] {
    c.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Shapes under the same name, in the order the first of them came, as objects.
fn group(pieces: Vec<Piece>) -> Vec<SceneObject> {
    let mut order: Vec<Vec<Piece>> = Vec::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for p in pieces {
        match (&p.scope, p.named) {
            (Some(name), true) => {
                if let Some(&i) = by_name.get(name) {
                    order[i].push(p);
                } else {
                    by_name.insert(name.clone(), order.len());
                    order.push(vec![p]);
                }
            }
            _ => order.push(vec![p]),
        }
    }
    order.into_iter().map(object).collect()
}

fn object(pieces: Vec<Piece>) -> SceneObject {
    let name = pieces[0].scope.clone();
    let color = pieces.iter().find_map(|p| p.material);
    // A colour per triangle when a shape has its own, or the shapes' colours differ.
    let several = pieces.iter().any(|p| p.colors.is_some())
        || pieces.iter().any(|p| p.material != pieces[0].material);
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    let mut colors = several.then(Vec::new);
    for p in pieces {
        let base = (positions.len() / 3) as u32;
        positions.extend(p.positions.iter().map(|&x| x as f32));
        for (t, tri) in p.triangles.iter().enumerate() {
            indices.extend(tri.map(|v| v + base));
            if let Some(out) = &mut colors {
                let c = p
                    .colors
                    .as_ref()
                    .map(|c| c[t])
                    .or(p.material)
                    .unwrap_or([204, 204, 204]);
                out.extend(c);
            }
        }
    }
    SceneObject {
        name,
        positions,
        indices,
        color,
        colors,
    }
}

// ---------------------------------------------------------------------------------------------
// Transforms

fn scaling(s: f64) -> Matrix {
    let mut m = crate::mesh::IDENTITY;
    m[0] = s;
    m[5] = s;
    m[10] = s;
    m
}

fn translation(t: [f64; 3]) -> Matrix {
    let mut m = crate::mesh::IDENTITY;
    m[12] = t[0];
    m[13] = t[1];
    m[14] = t[2];
    m
}

/// The rotation by `angle` (radians) about the axis `(x, y, z)`; identity for a zero axis.
fn rotation([x, y, z, angle]: [f64; 4]) -> Matrix {
    let length = (x * x + y * y + z * z).sqrt();
    if length == 0.0 || angle == 0.0 {
        return crate::mesh::IDENTITY;
    }
    let (x, y, z) = (x / length, y / length, z / length);
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    [
        t * x * x + c,
        t * x * y + s * z,
        t * x * z - s * y,
        0.0,
        t * x * y - s * z,
        t * y * y + c,
        t * y * z + s * x,
        0.0,
        t * x * z + s * y,
        t * y * z - s * x,
        t * z * z + c,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ]
}

/// A Transform's matrix: T · C · R · SR · S · SR⁻¹ · C⁻¹.
fn transform(node: &Node) -> Matrix {
    use crate::mesh::multiply;
    let t = node.vector("translation", [0.0; 3]);
    let c = node.vector("center", [0.0; 3]);
    let r = node.vector("rotation", [0.0, 0.0, 1.0, 0.0]);
    let s = node.vector("scale", [1.0; 3]);
    let so = node.vector("scaleOrientation", [0.0, 0.0, 1.0, 0.0]);
    let mut scale = crate::mesh::IDENTITY;
    scale[0] = s[0];
    scale[5] = s[1];
    scale[10] = s[2];
    let mut m = translation(t);
    m = multiply(&m, &translation(c));
    m = multiply(&m, &rotation(r));
    m = multiply(&m, &rotation(so));
    m = multiply(&m, &scale);
    m = multiply(&m, &rotation([so[0], so[1], so[2], -so[3]]));
    multiply(&m, &translation([-c[0], -c[1], -c[2]]))
}

// ---------------------------------------------------------------------------------------------
// Geometry

type Triangulated = (Vec<f64>, Vec<[u32; 3]>, Option<Vec<[u8; 3]>>);

/// The geometry's points (local, x y z), triangles and per-triangle colours; None for a kind that
/// is not read.
fn triangulate(g: &Node) -> Option<Triangulated> {
    let points = || -> Vec<f64> {
        g.child(&["Coordinate", "CoordinateDouble"])
            .and_then(|c| c.numbers("point"))
            .map(|p| p[..p.len() / 3 * 3].to_vec())
            .unwrap_or_default()
    };
    let ccw = g.flag("ccw", true);
    let out = match g.kind.as_str() {
        "IndexedFaceSet" => {
            let points = points();
            let index = g.numbers("coordIndex").unwrap_or(&[]);
            let convex = g.flag("convex", true);
            let mut tris = Vec::new();
            let mut polygon_of = Vec::new();
            let count = (points.len() / 3) as u32;
            for (f, poly) in polygons(index).enumerate() {
                let poly: Vec<u32> = poly
                    .iter()
                    .map(|&i| i as i64)
                    .filter(|&i| i >= 0 && (i as u32) < count)
                    .map(|i| i as u32)
                    .collect();
                let before = tris.len();
                polygon(&points, &poly, convex, &mut tris);
                polygon_of.extend(std::iter::repeat_n(f as u32, tris.len() - before));
            }
            let colors = face_set_colors(g, index, &tris, &polygon_of);
            (points, tris, colors)
        }
        "IndexedTriangleSet" | "IndexedTriangleFanSet" | "IndexedTriangleStripSet" => {
            let points = points();
            let index = g.numbers("index").unwrap_or(&[]);
            let count = (points.len() / 3) as u32;
            let valid = |i: f64| i >= 0.0 && (i as u32) < count;
            let mut tris = Vec::new();
            match g.kind.as_str() {
                "IndexedTriangleSet" => {
                    for t in index.as_chunks::<3>().0 {
                        if t.iter().all(|&i| valid(i)) {
                            tris.push([t[0] as u32, t[1] as u32, t[2] as u32]);
                        }
                    }
                }
                kind => {
                    for run in index.split(|&i| i < 0.0) {
                        let run: Vec<u32> = run
                            .iter()
                            .filter(|&&i| valid(i))
                            .map(|&i| i as u32)
                            .collect();
                        strip_or_fan(&run, kind.contains("Fan"), &mut tris);
                    }
                }
            }
            let colors = vertex_colors(g, &tris);
            (points, tris, colors)
        }
        "TriangleSet" | "TriangleFanSet" | "TriangleStripSet" => {
            let points = points();
            let count = (points.len() / 3) as u32;
            let mut tris = Vec::new();
            if g.kind == "TriangleSet" {
                for t in 0..count / 3 {
                    tris.push([3 * t, 3 * t + 1, 3 * t + 2]);
                }
            } else {
                let counts = g
                    .numbers(if g.kind == "TriangleFanSet" {
                        "fanCount"
                    } else {
                        "stripCount"
                    })
                    .unwrap_or(&[]);
                let mut at = 0u32;
                for &n in counts {
                    let n = n.max(0.0) as u32;
                    if at + n > count {
                        break;
                    }
                    let run: Vec<u32> = (at..at + n).collect();
                    strip_or_fan(&run, g.kind == "TriangleFanSet", &mut tris);
                    at += n;
                }
            }
            let colors = vertex_colors(g, &tris);
            (points, tris, colors)
        }
        "Box" => {
            let s = g.vector("size", [2.0; 3]);
            box_mesh(s)
        }
        "Sphere" => sphere_mesh(g.number("radius", 1.0)),
        "Cylinder" => {
            let r = g.number("radius", 1.0);
            let h = g.number("height", 2.0);
            let ends = [g.flag("bottom", true), g.flag("top", true)];
            revolve(&[[r, -h / 2.0], [r, h / 2.0]], ends)
        }
        "Cone" => {
            let r = g.number("bottomRadius", 1.0);
            let h = g.number("height", 2.0);
            revolve(
                &[[r, -h / 2.0], [0.0, h / 2.0]],
                [g.flag("bottom", true), false],
            )
        }
        _ => return None,
    };
    let (points, tris, colors) = out;
    let tris = if ccw {
        tris
    } else {
        tris.into_iter().map(|[a, b, c]| [a, c, b]).collect()
    };
    Some((points, tris, colors))
}

/// The polygons of an index list with −1 between them (the last may lack its −1).
fn polygons(index: &[f64]) -> impl Iterator<Item = &[f64]> {
    index.split(|&i| i < 0.0).filter(|p| p.len() >= 3)
}

/// A polygon's triangles: a fan, or for a polygon that may be concave, ear clipping in its plane.
fn polygon(points: &[f64], poly: &[u32], convex: bool, out: &mut Vec<[u32; 3]>) {
    if poly.len() < 3 {
        return;
    }
    if poly.len() == 3 || convex {
        for k in 1..poly.len() - 1 {
            out.push([poly[0], poly[k], poly[k + 1]]);
        }
        return;
    }
    let p = |i: u32| {
        let i = i as usize * 3;
        [points[i], points[i + 1], points[i + 2]]
    };
    // Newell's normal picks the plane to flatten onto.
    let mut n = [0.0f64; 3];
    for k in 0..poly.len() {
        let (a, b) = (p(poly[k]), p(poly[(k + 1) % poly.len()]));
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let axis = (0..3)
        .max_by(|&a, &b| n[a].abs().total_cmp(&n[b].abs()))
        .unwrap_or(2);
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let flat: Vec<f64> = poly.iter().flat_map(|&i| [p(i)[u], p(i)[v]]).collect();
    match earcutr::earcut(&flat, &[], 2) {
        Ok(tris) if tris.len() == 3 * (poly.len() - 2) => {
            // Ear clipping winds its own way: keep the polygon's.
            let keep = n[axis] >= 0.0;
            for t in tris.as_chunks::<3>().0 {
                let (a, b, c) = (poly[t[0]], poly[t[1]], poly[t[2]]);
                let area = (flat[2 * t[1]] - flat[2 * t[0]])
                    * (flat[2 * t[2] + 1] - flat[2 * t[0] + 1])
                    - (flat[2 * t[1] + 1] - flat[2 * t[0] + 1]) * (flat[2 * t[2]] - flat[2 * t[0]]);
                out.push(if (area >= 0.0) == keep {
                    [a, b, c]
                } else {
                    [a, c, b]
                });
            }
        }
        _ => {
            for k in 1..poly.len() - 1 {
                out.push([poly[0], poly[k], poly[k + 1]]);
            }
        }
    }
}

fn strip_or_fan(run: &[u32], fan: bool, out: &mut Vec<[u32; 3]>) {
    for k in 2..run.len() {
        out.push(if fan {
            [run[0], run[k - 1], run[k]]
        } else if k % 2 == 0 {
            [run[k - 2], run[k - 1], run[k]]
        } else {
            [run[k - 1], run[k - 2], run[k]]
        });
    }
}

/// The colours of a Color or ColorRGBA node, as RGB.
fn palette(g: &Node) -> Option<Vec<[u8; 3]>> {
    let c = g.child(&["Color", "ColorRGBA"])?;
    let width = if c.kind == "ColorRGBA" { 4 } else { 3 };
    let v = c.numbers("color")?;
    let colors: Vec<[u8; 3]> = v
        .chunks_exact(width)
        .map(|c| to_rgb([c[0], c[1], c[2]]))
        .collect();
    (!colors.is_empty()).then_some(colors)
}

fn mean(colors: [[u8; 3]; 3]) -> [u8; 3] {
    std::array::from_fn(|k| {
        ((colors[0][k] as u32 + colors[1][k] as u32 + colors[2][k] as u32 + 1) / 3) as u8
    })
}

/// Triangle colours of a triangle set: per vertex (the mean of the corners) or per triangle.
fn vertex_colors(g: &Node, tris: &[[u32; 3]]) -> Option<Vec<[u8; 3]>> {
    let palette = palette(g)?;
    let at = |i: usize| {
        palette
            .get(i)
            .copied()
            .unwrap_or(palette[palette.len() - 1])
    };
    Some(if g.flag("colorPerVertex", true) {
        tris.iter()
            .map(|t| mean(t.map(|v| at(v as usize))))
            .collect()
    } else {
        (0..tris.len()).map(at).collect()
    })
}

/// Triangle colours of an IndexedFaceSet, from colorIndex when there is one.
fn face_set_colors(
    g: &Node,
    coord_index: &[f64],
    tris: &[[u32; 3]],
    polygon_of: &[u32],
) -> Option<Vec<[u8; 3]>> {
    let palette = palette(g)?;
    let at = |i: f64| {
        let i = if i < 0.0 { 0 } else { i as usize };
        palette
            .get(i)
            .copied()
            .unwrap_or(palette[palette.len() - 1])
    };
    let color_index = g.numbers("colorIndex").filter(|c| !c.is_empty());
    if !g.flag("colorPerVertex", true) {
        return Some(
            polygon_of
                .iter()
                .map(|&f| {
                    at(color_index.map_or(f as f64, |c| c.get(f as usize).copied().unwrap_or(0.0)))
                })
                .collect(),
        );
    }
    let Some(ci) = color_index else {
        return Some(tris.iter().map(|t| mean(t.map(|v| at(v as f64)))).collect());
    };
    // colorIndex runs alongside coordIndex: each corner's colour by its place in the list.
    let mut corner: HashMap<(u32, u32), [u8; 3]> = HashMap::new();
    let mut f = 0u32;
    for (k, &i) in coord_index.iter().enumerate() {
        if i < 0.0 {
            f += 1;
            continue;
        }
        corner.insert((f, i as u32), at(ci.get(k).copied().unwrap_or(0.0)));
    }
    Some(
        tris.iter()
            .zip(polygon_of)
            .map(|(t, &f)| mean(t.map(|v| corner.get(&(f, v)).copied().unwrap_or(palette[0]))))
            .collect(),
    )
}

fn box_mesh(s: [f64; 3]) -> Triangulated {
    let h = s.map(|x| x / 2.0);
    let mut points = Vec::new();
    for k in 0..8 {
        points.extend([
            if k & 1 == 0 { -h[0] } else { h[0] },
            if k & 2 == 0 { -h[1] } else { h[1] },
            if k & 4 == 0 { -h[2] } else { h[2] },
        ]);
    }
    let quads = [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ];
    let tris = quads
        .iter()
        .flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]])
        .collect();
    (points, tris, None)
}

/// Segments round a circle for spheres, cylinders and cones.
const SEGMENTS: usize = 32;

fn sphere_mesh(r: f64) -> Triangulated {
    let rings = SEGMENTS / 2;
    let mut points = vec![0.0, r, 0.0];
    for i in 1..rings {
        let phi = std::f64::consts::PI * i as f64 / rings as f64;
        for j in 0..SEGMENTS {
            let theta = 2.0 * std::f64::consts::PI * j as f64 / SEGMENTS as f64;
            points.extend([
                r * phi.sin() * theta.sin(),
                r * phi.cos(),
                r * phi.sin() * theta.cos(),
            ]);
        }
    }
    points.extend([0.0, -r, 0.0]);
    let bottom = (points.len() / 3 - 1) as u32;
    let ring = |i: usize, j: usize| (1 + (i - 1) * SEGMENTS + j % SEGMENTS) as u32;
    let mut tris = Vec::new();
    for j in 0..SEGMENTS {
        tris.push([0, ring(1, j), ring(1, j + 1)]);
        tris.push([bottom, ring(rings - 1, j + 1), ring(rings - 1, j)]);
    }
    for i in 1..rings - 1 {
        for j in 0..SEGMENTS {
            let (a, b, c, d) = (
                ring(i, j),
                ring(i, j + 1),
                ring(i + 1, j),
                ring(i + 1, j + 1),
            );
            tris.push([a, c, d]);
            tris.push([a, d, b]);
        }
    }
    (points, tris, None)
}

/// A solid of revolution about Y through the profile points (radius, height), bottom to top, with
/// discs at the ends asked for.
fn revolve(profile: &[[f64; 2]], ends: [bool; 2]) -> Triangulated {
    let mut points = Vec::new();
    for &[r, y] in profile {
        for j in 0..SEGMENTS {
            let theta = 2.0 * std::f64::consts::PI * j as f64 / SEGMENTS as f64;
            points.extend([r * theta.sin(), y, r * theta.cos()]);
        }
    }
    let at = |i: usize, j: usize| (i * SEGMENTS + j % SEGMENTS) as u32;
    let mut tris = Vec::new();
    for i in 0..profile.len() - 1 {
        for j in 0..SEGMENTS {
            tris.push([at(i, j), at(i, j + 1), at(i + 1, j + 1)]);
            tris.push([at(i, j), at(i + 1, j + 1), at(i + 1, j)]);
        }
    }
    let last = profile.len() - 1;
    for (end, i, y, up) in [
        (ends[0], 0, profile[0][1], false),
        (ends[1], last, profile[last][1], true),
    ] {
        if !end || profile[i][0] == 0.0 {
            continue;
        }
        let centre = (points.len() / 3) as u32;
        points.extend([0.0, y, 0.0]);
        for j in 0..SEGMENTS {
            tris.push(if up {
                [centre, at(i, j), at(i, j + 1)]
            } else {
                [centre, at(i, j + 1), at(i, j)]
            });
        }
    }
    // Drop the triangles a cone's tip folds flat.
    let tris = tris
        .into_iter()
        .filter(|t| {
            let p = t.map(|v| {
                let v = v as usize * 3;
                [points[v], points[v + 1], points[v + 2]]
            });
            p[0] != p[1] && p[1] != p[2] && p[0] != p[2]
        })
        .collect();
    (points, tris, None)
}

/// Turns sRGB bytes into "#rrggbb".
pub fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(kind: &str, fields: Vec<(&str, Field)>) -> Rc<Node> {
        Rc::new(Node {
            kind: kind.into(),
            name: None,
            fields: fields
                .into_iter()
                .map(|(n, f)| (n.to_string(), f))
                .collect(),
        })
    }

    fn signed_volume(o: &SceneObject) -> f64 {
        o.indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let p = |i: u32| {
                    let i = i as usize * 3;
                    [
                        o.positions[i] as f64,
                        o.positions[i + 1] as f64,
                        o.positions[i + 2] as f64,
                    ]
                };
                let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
                (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                    + a[2] * (b[0] * c[1] - b[1] * c[0]))
                    / 6.0
            })
            .sum()
    }

    #[test]
    fn primitives_face_out_and_transforms_apply() {
        for (kind, volume, tolerance) in [
            ("Box", 8.0, 1e-9),
            ("Sphere", 4.0 / 3.0 * std::f64::consts::PI, 0.05),
            ("Cylinder", 2.0 * std::f64::consts::PI, 0.05),
            ("Cone", 2.0 / 3.0 * std::f64::consts::PI, 0.05),
        ] {
            let shape = node(
                "Shape",
                vec![("geometry", Field::Nodes(vec![node(kind, vec![])]))],
            );
            let t = node(
                "Transform",
                vec![
                    ("translation", Field::Numbers(vec![10.0, 0.0, 0.0])),
                    ("scale", Field::Numbers(vec![2.0, 2.0, 2.0])),
                    ("children", Field::Nodes(vec![shape])),
                ],
            );
            let mut warnings = Vec::new();
            let objects = collect(&[t], 1.0, &mut warnings);
            assert_eq!(objects.len(), 1, "{kind}");
            let v = signed_volume(&objects[0]);
            assert!(
                (v - 8.0 * volume).abs() < 8.0 * volume * tolerance,
                "{kind}: {v}"
            );
            let xs: Vec<f32> = objects[0].positions.iter().step_by(3).copied().collect();
            let mid = (xs.iter().cloned().fold(f32::MAX, f32::min)
                + xs.iter().cloned().fold(f32::MIN, f32::max))
                / 2.0;
            assert!((mid - 10.0).abs() < 1e-4, "{kind}: centre {mid}");
        }
    }

    #[test]
    fn a_mirroring_transform_keeps_the_box_facing_out() {
        let shape = node(
            "Shape",
            vec![("geometry", Field::Nodes(vec![node("Box", vec![])]))],
        );
        let t = node(
            "Transform",
            vec![
                ("scale", Field::Numbers(vec![-1.0, 1.0, 1.0])),
                ("children", Field::Nodes(vec![shape])),
            ],
        );
        let objects = collect(&[t], 1.0, &mut Vec::new());
        assert!((signed_volume(&objects[0]) - 8.0).abs() < 1e-9);
    }

    #[test]
    fn rotations_follow_the_right_hand_rule() {
        // 90° about Z takes X to Y.
        let m = rotation([0.0, 0.0, 1.0, std::f64::consts::FRAC_PI_2]);
        let p = crate::mesh::transform_point(&m, [1.0, 0.0, 0.0]);
        assert!((p[0]).abs() < 1e-12 && (p[1] - 1.0).abs() < 1e-12, "{p:?}");
    }

    #[test]
    fn a_concave_polygon_is_ear_clipped_and_keeps_its_winding() {
        // An L shape in the XY plane, counter-clockwise seen from +Z.
        let points = vec![
            0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 2.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 2.0, 0.0, 0.0, 2.0,
            0.0,
        ];
        let mut tris = Vec::new();
        polygon(&points, &[0, 1, 2, 3, 4, 5], false, &mut tris);
        assert_eq!(tris.len(), 4);
        let area: f64 = tris
            .iter()
            .map(|t| {
                let p = |i: u32| (points[3 * i as usize], points[3 * i as usize + 1]);
                let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
                ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)) / 2.0
            })
            .sum();
        assert!((area - 3.0).abs() < 1e-12, "{area}");
    }
}
