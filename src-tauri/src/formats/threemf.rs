//! 3MF, following the 3MF Core specification: the root model is found through _rels/.rels,
//! elements may use any namespace prefix, all six units are converted to millimetres, objects hold
//! a mesh or components (recursive, transforms composed, loops rejected), and each build item
//! becomes one part. Components and items that point into another model file with the
//! production extension's `p:path` (as Bambu Studio and PrusaSlicer write) are followed.
//!
//! The same reader opens tricky3D projects (src/project.rs), which add metadata on top.

use std::collections::HashMap;
use std::io::{self, BufWriter, Seek, Write};

use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::xml::{
    Budget, append_text, attribute, entries_ending, find_entry, index, number, open_zip,
    read_entry, unit_scale, utf8, xml_error,
};
use super::{Loaded, NamedMesh, Object};
use crate::mesh::{
    IDENTITY, Matrix, Mesh, fmt_count, linear_determinant, multiply, transform_point, weld_indexed,
};
use crate::progress::{Progress, Stage};

pub const CORE: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
pub const PRODUCTION: &str = "http://schemas.microsoft.com/3dmanufacturing/production/2015/06";
const MATERIAL: &str = "http://schemas.microsoft.com/3dmanufacturing/material/2015/02";
const MODEL_RELATIONSHIP: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
const DEFAULT_MODEL: &str = "3D/3dmodel.model";

/// Metadata entries as (name, value). A name whose prefix the model declares becomes
/// "{namespace}local", so "tricky3d:platform" reads back whatever prefix another writer chose.
pub type Metadata = Vec<(String, String)>;

/// One build item, ready to become a part.
#[derive(Debug)]
pub struct Item {
    /// The `name` of the object the item places.
    pub object_name: Option<String>,
    /// The item's transform with the unit applied to its translation.
    pub matrix: Matrix,
    /// The object's mesh with component transforms and the unit baked in, welded.
    pub mesh: Mesh,
    /// The object's `<metadatagroup>` entries.
    pub metadata: Metadata,
}

#[derive(Debug)]
pub struct Package {
    pub items: Vec<Item>,
    /// Model-level `<metadata>` of the root model.
    pub metadata: Metadata,
    pub warnings: Vec<String>,
}

impl Package {
    pub fn into_loaded(self) -> Loaded {
        let object_count = self.items.len();
        let objects = self
            .items
            .into_iter()
            .map(|item| Object {
                name: item.object_name,
                matrix: Some(item.matrix),
                mesh: item.mesh,
            })
            .collect();
        Loaded {
            objects,
            object_count,
            splittable: false,
            warnings: self.warnings,
        }
    }

    pub fn metadata_value(&self, key: &str) -> Option<&str> {
        self.metadata
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Reads a 3MF package (a zip) into one welded mesh per build item.
pub fn read(bytes: &[u8], progress: &mut Progress) -> Result<Package, String> {
    let mut zip = open_zip(bytes)?;
    let root = root_model_name(&mut zip)?;
    let root_key = part_key(&root);
    let total: u64 = entries_ending(&mut zip, ".model")
        .iter()
        .map(|(_, size)| size)
        .sum::<u64>()
        .max(1);

    // Load the root model and every model file it reaches through p:path. One budget counts what
    // all of them hold, so splitting a huge mesh over several files does not get past it.
    let mut models: HashMap<String, Model> = HashMap::new();
    let mut queue = vec![root.clone()];
    let mut parsed = 0u64;
    let mut held = Budget::default();
    while let Some(name) = queue.pop() {
        let key = part_key(&name);
        if models.contains_key(&key) {
            continue;
        }
        let actual = find_entry(&zip, &name).ok_or_else(|| {
            format!(
                "it refers to a model part that is missing ({})",
                name.trim_start_matches('/')
            )
        })?;
        let data = read_entry(&mut zip, &actual)?;
        let size = data.len() as u64;
        let text = utf8(&data, "3D model")?;
        let model = parse_model(text, &mut held, &mut |f| {
            progress.report(
                Stage::Parse,
                (parsed as f64 + f * size as f64) / total as f64,
            )
        })?;
        parsed += size;
        queue.extend(model.referenced_paths().map(str::to_string));
        models.insert(key, model);
    }
    let Some(root_model) = models.get(&root_key) else {
        return Err("its 3D model is missing".into());
    };

    let mut warnings = Vec::new();
    let scale = match root_model.unit.as_deref() {
        None => 1.0,
        Some(unit) => unit_scale(unit).unwrap_or_else(|| {
            warnings.push(format!(
                "The unit \"{unit}\" is unknown, so the sizes were read as millimetres."
            ));
            1.0
        }),
    };
    for ns in &root_model.required {
        if !matches!(ns.as_str(), CORE | PRODUCTION | MATERIAL) {
            warnings.push(format!(
                "The file needs a 3MF extension tricky3D does not support ({ns}); only the plain meshes were loaded."
            ));
        }
    }
    // An empty build is valid 3MF (an empty tricky3D project); importers report "no triangles".
    let mut items = Vec::with_capacity(root_model.build.len());
    // A second budget for what the build expands to once components repeat their meshes.
    let mut budget = Budget::default();
    let mut sizes = HashMap::new();
    for (n, item) in root_model.build.iter().enumerate() {
        let path = item
            .path
            .as_deref()
            .map(part_key)
            .unwrap_or_else(|| root_key.clone());
        let object = models
            .get(&path)
            .and_then(|m| m.objects.get(&item.object))
            .ok_or_else(|| {
                format!(
                    "its build refers to object {}, which does not exist",
                    item.object
                )
            })?;
        let size = measure(&models, &path, item.object, &mut Vec::new(), &mut sizes)?;
        budget.visit(size.uses)?;
        budget.add(size.vertices, size.triangles, "would have")?;
        let mut pieces = Vec::new();
        flatten(&models, &path, item.object, IDENTITY, &mut pieces);
        let mesh = bake_pieces(&pieces, scale)?;
        let mut matrix = item.transform;
        for t in &mut matrix[12..15] {
            *t *= scale;
        }
        items.push(Item {
            object_name: object.name.clone(),
            matrix,
            mesh,
            metadata: object.metadata.clone(),
        });
        progress.report(Stage::Weld, (n + 1) as f64 / root_model.build.len() as f64);
    }
    Ok(Package {
        items,
        metadata: root_model.metadata.clone(),
        warnings,
    })
}

/// Zip entry names compare without case or a leading slash.
fn part_key(name: &str) -> String {
    name.trim_start_matches('/').to_ascii_lowercase()
}

/// The root model's part name from _rels/.rels, or the conventional name.
fn root_model_name(zip: &mut super::xml::Zip) -> Result<String, String> {
    if let Some(rels) = find_entry(zip, "_rels/.rels") {
        let data = read_entry(zip, &rels)?;
        let text = utf8(&data, "package relationships")?;
        let mut reader = NsReader::from_str(text);
        loop {
            match reader.read_event() {
                Ok(Event::Start(e) | Event::Empty(e))
                    if e.local_name().as_ref() == "Relationship" =>
                {
                    let (mut target, mut kind) = (None, None);
                    for attr in e.attributes().flatten() {
                        match attr.key.as_ref() {
                            "Target" => target = Some(attr.value.to_string()),
                            "Type" => kind = Some(attr.value.to_string()),
                            _ => {}
                        }
                    }
                    if let (Some(target), Some(MODEL_RELATIONSHIP)) = (target, kind.as_deref())
                        && find_entry(zip, &target).is_some()
                    {
                        return Ok(target);
                    }
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
    }
    find_entry(zip, DEFAULT_MODEL)
        .ok_or_else(|| "it is not a 3MF file (it has no 3D model)".to_string())
}

// ---------------------------------------------------------------------------------------------
// Model files

#[derive(Debug, Default)]
struct RawMesh {
    positions: Vec<f64>,
    indices: Vec<u32>,
}

#[derive(Debug)]
enum Content {
    Mesh(RawMesh),
    Components(Vec<Component>),
    Empty,
}

#[derive(Debug)]
struct Component {
    object: u32,
    path: Option<String>,
    transform: Matrix,
}

impl Component {
    /// The model file of the object it places; without p:path, the one it is in.
    fn model_path(&self, parent: &str) -> String {
        self.path
            .as_deref()
            .map(part_key)
            .unwrap_or_else(|| parent.to_string())
    }
}

#[derive(Debug)]
struct ObjectDef {
    name: Option<String>,
    content: Content,
    metadata: Metadata,
}

#[derive(Debug)]
struct BuildItem {
    object: u32,
    path: Option<String>,
    transform: Matrix,
}

#[derive(Debug, Default)]
struct Model {
    unit: Option<String>,
    /// Namespaces of the `requiredextensions` prefixes.
    required: Vec<String>,
    objects: HashMap<u32, ObjectDef>,
    build: Vec<BuildItem>,
    metadata: Metadata,
}

impl Model {
    fn referenced_paths(&self) -> impl Iterator<Item = &str> {
        let components = self.objects.values().flat_map(|o| match &o.content {
            Content::Components(list) => list
                .iter()
                .filter_map(|c| c.path.as_deref())
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        });
        components.chain(self.build.iter().filter_map(|b| b.path.as_deref()))
    }
}

/// Where an element sits, for the elements tricky3D reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tag {
    Model,
    ModelMetadata,
    Resources,
    Object,
    MetadataGroup,
    ObjectMetadata,
    Mesh,
    Vertices,
    Vertex,
    Triangles,
    Triangle,
    Components,
    Component,
    Build,
    Item,
    Other,
}

fn tag_for(parent: Option<Tag>, local: &str) -> Tag {
    match (parent, local) {
        (None, "model") => Tag::Model,
        (Some(Tag::Model), "metadata") => Tag::ModelMetadata,
        (Some(Tag::Model), "resources") => Tag::Resources,
        (Some(Tag::Resources), "object") => Tag::Object,
        (Some(Tag::Object), "metadatagroup") => Tag::MetadataGroup,
        (Some(Tag::MetadataGroup), "metadata") => Tag::ObjectMetadata,
        (Some(Tag::Object), "mesh") => Tag::Mesh,
        (Some(Tag::Mesh), "vertices") => Tag::Vertices,
        (Some(Tag::Vertices), "vertex") => Tag::Vertex,
        (Some(Tag::Mesh), "triangles") => Tag::Triangles,
        (Some(Tag::Triangles), "triangle") => Tag::Triangle,
        (Some(Tag::Object), "components") => Tag::Components,
        (Some(Tag::Components), "component") => Tag::Component,
        (Some(Tag::Model), "build") => Tag::Build,
        (Some(Tag::Build), "item") => Tag::Item,
        _ => Tag::Other,
    }
}

/// State while one model file is parsed.
#[derive(Default)]
struct ModelParser {
    model: Model,
    /// Prefix → namespace, from the `<model>` element.
    prefixes: HashMap<String, String>,
    object: Option<(u32, ObjectDef)>,
    mesh: RawMesh,
    components: Vec<Component>,
    /// (name, text so far) of the `<metadata>` being read.
    metadata: Option<(String, String)>,
}

fn parse_model(
    text: &str,
    budget: &mut Budget,
    progress: &mut dyn FnMut(f64),
) -> Result<Model, String> {
    let mut reader = NsReader::from_str(text);
    let mut parser = ModelParser::default();
    let mut stack: Vec<Tag> = Vec::new();
    let mut elements = 0usize;
    loop {
        let event = reader.read_event().map_err(|e| xml_error("3D model", &e))?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                let (ns, local) = reader.resolver().resolve_element(e.name());
                let core = match ns {
                    ResolveResult::Bound(Namespace(n)) => n == CORE,
                    // Some writers leave out the namespace declaration; read those as core.
                    ResolveResult::Unbound => true,
                    ResolveResult::Unknown(_) => false,
                };
                let tag = if core {
                    tag_for(stack.last().copied(), local.as_ref())
                } else {
                    Tag::Other
                };
                let is_start = matches!(event, Event::Start(_));
                if tag == Tag::Other {
                    if is_start {
                        // An extension or unknown element: skip it with everything inside.
                        reader
                            .read_to_end(e.name())
                            .map_err(|e| xml_error("3D model", &e))?;
                    }
                    continue;
                }
                parser.start(tag, e, &reader, budget)?;
                if is_start {
                    stack.push(tag);
                } else {
                    parser.end(tag)?;
                }
                elements += 1;
                if elements.is_multiple_of(65_536) {
                    progress(reader.buffer_position() as f64 / text.len().max(1) as f64);
                }
            }
            Event::End(_) => {
                if let Some(tag) = stack.pop() {
                    parser.end(tag)?;
                }
            }
            Event::Text(_) | Event::CData(_) | Event::GeneralRef(_) => {
                if let Some((_, value)) = parser.metadata.as_mut() {
                    append_text(value, &event);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    progress(1.0);
    Ok(parser.model)
}

fn bad(what: &str) -> String {
    format!("its 3D model has {what}")
}

impl ModelParser {
    fn start(
        &mut self,
        tag: Tag,
        e: &BytesStart,
        reader: &NsReader<&[u8]>,
        budget: &mut Budget,
    ) -> Result<(), String> {
        match tag {
            Tag::Model => {
                for attr in e.attributes().flatten() {
                    let key = attr.key.as_ref();
                    if let Some(prefix) = key.strip_prefix("xmlns:") {
                        self.prefixes
                            .insert(prefix.to_string(), attr.value.to_string());
                    } else if key == "unit" {
                        self.model.unit = Some(attr.value.trim().to_string());
                    } else if key == "requiredextensions" {
                        let required: Vec<String> =
                            attr.value.split_whitespace().map(str::to_string).collect();
                        self.model.required = required;
                    }
                }
                // requiredextensions lists prefixes; keep their namespaces.
                let prefixes = &self.prefixes;
                self.model.required = self
                    .model
                    .required
                    .iter()
                    .map(|p| prefixes.get(p).cloned().unwrap_or_else(|| p.clone()))
                    .collect();
            }
            Tag::ModelMetadata | Tag::ObjectMetadata => {
                let name = attribute(e, "name").unwrap_or_default();
                self.metadata = Some((self.metadata_key(&name), String::new()));
            }
            Tag::Object => {
                let id = attribute(e, "id")
                    .as_deref()
                    .and_then(index)
                    .ok_or_else(|| bad("an object without a valid id"))?;
                let name = attribute(e, "name");
                self.object = Some((
                    id,
                    ObjectDef {
                        name,
                        content: Content::Empty,
                        metadata: Vec::new(),
                    },
                ));
            }
            Tag::Mesh => self.mesh = RawMesh::default(),
            Tag::Vertex => {
                budget.add(1, 0, "has")?;
                let mut p = [None; 3];
                for attr in e.attributes().with_checks(false).flatten() {
                    let axis = match attr.key.as_ref() {
                        "x" => 0,
                        "y" => 1,
                        "z" => 2,
                        _ => continue,
                    };
                    p[axis] = number(&attr.value);
                }
                let [Some(x), Some(y), Some(z)] = p else {
                    return Err(bad(&format!(
                        "a vertex that is not a number (vertex {})",
                        fmt_count(self.mesh.positions.len() as u64 / 3 + 1)
                    )));
                };
                self.mesh.positions.extend_from_slice(&[x, y, z]);
            }
            Tag::Triangle => {
                budget.add(0, 1, "has")?;
                let mut v = [None; 3];
                for attr in e.attributes().with_checks(false).flatten() {
                    let k = match attr.key.as_ref() {
                        "v1" => 0,
                        "v2" => 1,
                        "v3" => 2,
                        _ => continue,
                    };
                    v[k] = index(&attr.value);
                }
                let [Some(a), Some(b), Some(c)] = v else {
                    return Err(bad(&format!(
                        "a triangle without valid corners (triangle {})",
                        fmt_count(self.mesh.indices.len() as u64 / 3 + 1)
                    )));
                };
                self.mesh.indices.extend_from_slice(&[a, b, c]);
            }
            Tag::Components => self.components.clear(),
            Tag::Component => {
                budget.visit(1)?;
                let (object, path, transform) = placement(e, reader)?;
                self.components.push(Component {
                    object,
                    path,
                    transform,
                });
            }
            Tag::Item => {
                budget.visit(1)?;
                let (object, path, transform) = placement(e, reader)?;
                self.model.build.push(BuildItem {
                    object,
                    path,
                    transform,
                });
            }
            Tag::Resources
            | Tag::MetadataGroup
            | Tag::Vertices
            | Tag::Triangles
            | Tag::Build
            | Tag::Other => {}
        }
        Ok(())
    }

    fn end(&mut self, tag: Tag) -> Result<(), String> {
        match tag {
            Tag::ModelMetadata | Tag::ObjectMetadata => {
                if let Some((name, value)) = self.metadata.take() {
                    let entry = (name, value.trim().to_string());
                    match (tag, self.object.as_mut()) {
                        (Tag::ObjectMetadata, Some((_, object))) => object.metadata.push(entry),
                        _ => self.model.metadata.push(entry),
                    }
                }
            }
            Tag::Mesh => {
                let mesh = std::mem::take(&mut self.mesh);
                let count = mesh.positions.len() / 3;
                if mesh.indices.iter().any(|&i| i as usize >= count) {
                    return Err(bad(
                        "a triangle that refers to a vertex that does not exist",
                    ));
                }
                if let Some((_, object)) = self.object.as_mut() {
                    object.content = Content::Mesh(mesh);
                }
            }
            Tag::Components => {
                if let Some((_, object)) = self.object.as_mut() {
                    object.content = Content::Components(std::mem::take(&mut self.components));
                }
            }
            Tag::Object => {
                if let Some((id, object)) = self.object.take()
                    && self.model.objects.insert(id, object).is_some()
                {
                    return Err(bad(&format!("two objects with the id {id}")));
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// "{namespace}local" for a prefixed name whose prefix the model declares.
    fn metadata_key(&self, name: &str) -> String {
        match name.split_once(':') {
            Some((prefix, local)) => match self.prefixes.get(prefix) {
                Some(ns) => format!("{{{ns}}}{local}"),
                None => name.to_string(),
            },
            None => name.to_string(),
        }
    }
}

/// objectid, p:path and transform of a component or build item.
fn placement(
    e: &BytesStart,
    reader: &NsReader<&[u8]>,
) -> Result<(u32, Option<String>, Matrix), String> {
    let mut object = None;
    let mut path = None;
    let mut transform = IDENTITY;
    for attr in e.attributes().flatten() {
        let (ns, local) = reader.resolver().resolve_attribute(attr.key);
        match (ns, local.as_ref()) {
            (ResolveResult::Unbound, "objectid") => object = index(&attr.value),
            (ResolveResult::Unbound, "transform") => {
                transform = parse_transform(&attr.value)
                    .ok_or_else(|| bad("a transform that is not 12 numbers"))?;
            }
            (ResolveResult::Bound(Namespace(PRODUCTION)), "path") => {
                path = Some(attr.value.trim().to_string())
            }
            _ => {}
        }
    }
    let object =
        object.ok_or_else(|| bad("a component or build item without a valid object id"))?;
    Ok((object, path, transform))
}

/// The 3MF `transform` "m00 m01 m02 m10 m11 m12 m20 m21 m22 m30 m31 m32" (row-vector
/// convention) as a column-major matrix: each 3MF row is one column.
pub fn parse_transform(text: &str) -> Option<Matrix> {
    let values: Vec<f64> = text.split_whitespace().map(number).collect::<Option<_>>()?;
    let [m00, m01, m02, m10, m11, m12, m20, m21, m22, m30, m31, m32] = values.as_slice() else {
        return None;
    };
    Some([
        *m00, *m01, *m02, 0.0, *m10, *m11, *m12, 0.0, *m20, *m21, *m22, 0.0, *m30, *m31, *m32, 1.0,
    ])
}

/// The inverse of `parse_transform`, in shortest round-trip number form.
pub fn format_transform(m: &Matrix) -> String {
    [
        m[0], m[1], m[2], m[4], m[5], m[6], m[8], m[9], m[10], m[12], m[13], m[14],
    ]
    .iter()
    .map(|v| v.to_string())
    .collect::<Vec<_>>()
    .join(" ")
}

/// What one use of an object adds up to once its components repeat their meshes.
#[derive(Clone, Copy)]
struct Size {
    uses: u64,
    vertices: u64,
    triangles: u64,
    /// Component levels below the object.
    levels: usize,
}

/// Sizes an object without repeating anything: each distinct object is measured once and its
/// size reused, so a file whose components multiply past the limits is refused in time that
/// grows with the file, not with what it would expand to. Also rejects loops, nesting deeper
/// than 32 levels and components of missing objects, so `flatten` can follow the components.
fn measure(
    models: &HashMap<String, Model>,
    path: &str,
    id: u32,
    stack: &mut Vec<(String, u32)>,
    sizes: &mut HashMap<(String, u32), Size>,
) -> Result<Size, String> {
    if stack.iter().any(|(p, i)| p == path && *i == id) {
        return Err("its components refer to each other in a loop".into());
    }
    let key = (path.to_string(), id);
    let known = sizes.get(&key).copied();
    if stack.len() + known.map_or(0, |s| s.levels) > 32 {
        return Err("its components are nested more than 32 levels deep".into());
    }
    if let Some(size) = known {
        return Ok(size);
    }
    let object = models
        .get(path)
        .and_then(|m| m.objects.get(&id))
        .ok_or_else(|| format!("a component refers to object {id}, which does not exist"))?;
    let mut size = Size {
        uses: 1,
        vertices: 0,
        triangles: 0,
        levels: 0,
    };
    match &object.content {
        Content::Mesh(mesh) => {
            size.vertices = mesh.positions.len() as u64 / 3;
            size.triangles = mesh.indices.len() as u64 / 3;
        }
        Content::Components(list) => {
            stack.push(key.clone());
            for c in list {
                let child = measure(models, &c.model_path(path), c.object, stack, sizes)?;
                size.uses = size.uses.saturating_add(child.uses);
                size.vertices = size.vertices.saturating_add(child.vertices);
                size.triangles = size.triangles.saturating_add(child.triangles);
                size.levels = size.levels.max(child.levels + 1);
            }
            stack.pop();
        }
        Content::Empty => {}
    }
    sizes.insert(key, size);
    Ok(size)
}

/// Collects the meshes an object is made of, each with its accumulated component transform.
/// Only for an object `measure` has accepted.
fn flatten<'m>(
    models: &'m HashMap<String, Model>,
    path: &str,
    id: u32,
    matrix: Matrix,
    out: &mut Vec<(Matrix, &'m RawMesh)>,
) {
    match &models[path].objects[&id].content {
        Content::Mesh(mesh) => out.push((matrix, mesh)),
        Content::Components(list) => {
            for c in list {
                let matrix = multiply(&matrix, &c.transform);
                flatten(models, &c.model_path(path), c.object, matrix, out);
            }
        }
        Content::Empty => {}
    }
}

/// Bakes each piece's transform and the unit into f32 vertices, then welds the whole object.
fn bake_pieces(pieces: &[(Matrix, &RawMesh)], scale: f64) -> Result<Mesh, String> {
    let vertices: usize = pieces.iter().map(|(_, m)| m.positions.len()).sum();
    let corners: usize = pieces.iter().map(|(_, m)| m.indices.len()).sum();
    let mut positions = Vec::with_capacity(vertices);
    let mut indices = Vec::with_capacity(corners);
    for (matrix, mesh) in pieces {
        let base = u32::try_from(positions.len() / 3)
            .map_err(|_| "it has more vertices than tricky3D can handle".to_string())?;
        let identity = *matrix == IDENTITY;
        for p in mesh.positions.as_chunks::<3>().0 {
            let q = if identity {
                [p[0], p[1], p[2]]
            } else {
                transform_point(matrix, [p[0], p[1], p[2]])
            };
            positions.extend_from_slice(&[
                (q[0] * scale) as f32,
                (q[1] * scale) as f32,
                (q[2] * scale) as f32,
            ]);
        }
        let flip = linear_determinant(matrix) < 0.0;
        for t in mesh.indices.as_chunks::<3>().0 {
            let (b, c) = if flip { (t[2], t[1]) } else { (t[1], t[2]) };
            indices.extend_from_slice(&[base + t[0], base + b, base + c]);
        }
    }
    weld_indexed(&positions, &indices)
}

// ---------------------------------------------------------------------------------------------
// Writing

pub struct ObjectOut<'a> {
    pub name: &'a str,
    pub mesh: &'a Mesh,
    /// Build item transform; None writes the item without one (identity).
    pub transform: Option<Matrix>,
    /// `<metadatagroup>` entries (names must use a declared prefix).
    pub metadata: Vec<(&'a str, String)>,
}

pub struct ModelOut<'a> {
    /// Extra (prefix, namespace) declarations on `<model>`.
    pub namespaces: &'a [(&'a str, &'a str)],
    pub metadata: Vec<(&'a str, String)>,
    pub objects: Vec<ObjectOut<'a>>,
}

pub fn application() -> String {
    format!("tricky3D {}", env!("CARGO_PKG_VERSION"))
}

/// Export 3MF: unit millimetre, one object per part, the transforms already baked.
pub fn write_export<W: Write + Seek>(w: W, parts: &[NamedMesh]) -> io::Result<W> {
    let objects = parts
        .iter()
        .map(|p| ObjectOut {
            name: p.name,
            mesh: p.mesh,
            transform: None,
            metadata: Vec::new(),
        })
        .collect();
    write(
        w,
        &ModelOut {
            namespaces: &[],
            metadata: vec![("Application", application())],
            objects,
        },
    )
}

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/></Types>
"#;

const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>
"#;

/// Writes a complete 3MF package: content types, relationships and the model.
pub fn write<W: Write + Seek>(w: W, model: &ModelOut) -> io::Result<W> {
    let mut zip = ZipWriter::new(w);
    let estimate: u64 = model
        .objects
        .iter()
        .map(|o| o.mesh.vertex_count() as u64 * 64 + o.mesh.triangle_count() as u64 * 48)
        .sum();
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file("[Content_Types].xml", options)
        .map_err(io::Error::other)?;
    zip.write_all(CONTENT_TYPES.as_bytes())?;
    zip.start_file("_rels/.rels", options)
        .map_err(io::Error::other)?;
    zip.write_all(RELS.as_bytes())?;
    zip.start_file(DEFAULT_MODEL, options.large_file(estimate > 3 << 30))
        .map_err(io::Error::other)?;
    {
        let mut out = BufWriter::with_capacity(1 << 20, &mut zip);
        write_model(&mut out, model)?;
        out.flush()?;
    }
    zip.finish().map_err(io::Error::other)
}

fn write_model(w: &mut impl Write, model: &ModelOut) -> io::Result<()> {
    use super::xml::escape;
    write!(
        w,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"{CORE}\""
    )?;
    for (prefix, ns) in model.namespaces {
        write!(w, " xmlns:{prefix}=\"{}\"", escape(ns))?;
    }
    writeln!(w, ">")?;
    for (name, value) in &model.metadata {
        writeln!(
            w,
            " <metadata name=\"{}\">{}</metadata>",
            escape(name),
            escape(value)
        )?;
    }
    writeln!(w, " <resources>")?;
    for (k, o) in model.objects.iter().enumerate() {
        writeln!(
            w,
            "  <object id=\"{}\" type=\"model\" name=\"{}\">",
            k + 1,
            escape(o.name)
        )?;
        if !o.metadata.is_empty() {
            writeln!(w, "   <metadatagroup>")?;
            for (name, value) in &o.metadata {
                writeln!(
                    w,
                    "    <metadata name=\"{}\">{}</metadata>",
                    escape(name),
                    escape(value)
                )?;
            }
            writeln!(w, "   </metadatagroup>")?;
        }
        writeln!(w, "   <mesh>\n    <vertices>")?;
        for v in o.mesh.positions.as_chunks::<3>().0 {
            writeln!(
                w,
                "     <vertex x=\"{}\" y=\"{}\" z=\"{}\"/>",
                v[0], v[1], v[2]
            )?;
        }
        writeln!(w, "    </vertices>\n    <triangles>")?;
        for t in o.mesh.indices.as_chunks::<3>().0 {
            writeln!(
                w,
                "     <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>",
                t[0], t[1], t[2]
            )?;
        }
        writeln!(w, "    </triangles>\n   </mesh>\n  </object>")?;
    }
    writeln!(w, " </resources>\n <build>")?;
    for (k, o) in model.objects.iter().enumerate() {
        match &o.transform {
            Some(m) => writeln!(
                w,
                "  <item objectid=\"{}\" transform=\"{}\"/>",
                k + 1,
                format_transform(m)
            )?,
            None => writeln!(w, "  <item objectid=\"{}\"/>", k + 1)?,
        }
    }
    writeln!(w, " </build>\n</model>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_maps_rows_to_columns_and_back() {
        let m = parse_transform("1 2 3 4 5 6 7 8 9 10 11 12").unwrap();
        assert_eq!(
            m,
            [
                1.0, 2.0, 3.0, 0.0, 4.0, 5.0, 6.0, 0.0, 7.0, 8.0, 9.0, 0.0, 10.0, 11.0, 12.0, 1.0
            ]
        );
        assert_eq!(format_transform(&m), "1 2 3 4 5 6 7 8 9 10 11 12");
        assert!(parse_transform("1 2 3").is_none());
        assert!(parse_transform("1 2 3 4 5 6 7 8 9 10 11 nan").is_none());
    }

    /// Object 1 is a mesh; object 2 places it with one component; the build places object 2.
    fn model(vertices: usize, triangles: usize) -> String {
        format!(
            r#"<model xmlns="{CORE}"><resources><object id="1"><mesh><vertices>{}</vertices><triangles>{}</triangles></mesh></object><object id="2"><components><component objectid="1"/></components></object></resources><build><item objectid="2"/></build></model>"#,
            r#"<vertex x="0" y="0" z="0"/>"#.repeat(vertices),
            r#"<triangle v1="0" v2="0" v3="0"/>"#.repeat(triangles),
        )
    }

    /// One budget counts every model file of a package; these start it where earlier files left it.
    #[test]
    fn limits_stop_reading_at_the_element_that_crosses_them() {
        use crate::formats::xml::{MAX_TRIANGLES, MAX_VERTICES, MAX_VISITS};
        let mut budget = Budget {
            vertices: MAX_VERTICES - 4,
            ..Budget::default()
        };
        parse_model(&model(3, 1), &mut budget, &mut |_| {}).unwrap();
        let err = parse_model(&model(3, 1), &mut budget, &mut |_| {}).unwrap_err();
        assert_eq!(
            err,
            "it has more than 100,000,000 vertices, which is more than tricky3D can handle"
        );
        assert_eq!(
            budget.vertices,
            MAX_VERTICES + 1,
            "stops at the vertex over"
        );

        let mut budget = Budget {
            triangles: MAX_TRIANGLES - 1,
            ..Budget::default()
        };
        let err = parse_model(&model(3, 2), &mut budget, &mut |_| {}).unwrap_err();
        assert_eq!(
            err,
            "it has more than 50,000,000 triangles, which is more than tricky3D can handle"
        );
        assert_eq!(budget.triangles, MAX_TRIANGLES + 1);

        // Each component and build item is one use of an object.
        let mut budget = Budget {
            visits: MAX_VISITS - 1,
            ..Budget::default()
        };
        let err = parse_model(&model(3, 1), &mut budget, &mut |_| {}).unwrap_err();
        assert_eq!(err, "it repeats its components too many times");

        let mut budget = Budget::default();
        parse_model(&model(3, 1), &mut budget, &mut |_| {}).unwrap();
        assert_eq!(
            (budget.vertices, budget.triangles, budget.visits),
            (3, 1, 2)
        );
    }
}
