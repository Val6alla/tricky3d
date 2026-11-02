//! X3D in its XML encoding (.x3d, gzip-compressed .x3dz), read into the scene graph of `scene`:
//! each element a node, each attribute a field (numbers, true or false, or text), DEF and USE
//! followed, child elements filed under their containerField (else "children"). The head's
//! `<unit category="length">` sets the length unit; without one the numbers are millimetres, as 3D
//! printing tools write them (the standard says metres). Prototypes, scripts and ROUTEs are skipped;
//! a ProtoInstance gives no geometry (a warning says so).
//!
//! Export writes X3D 3.3 with a millimetre unit statement, one Shape per part holding an
//! IndexedTriangleSet.

use std::collections::HashMap;
use std::io::{self, Write};
use std::rc::Rc;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use super::scene::{Field, Node};
use super::xml::{attribute, escape, utf8, xml_error};
use super::{Loaded, NamedMesh};
use crate::progress::{Progress, Stage};

/// Elements whose content is never part of the scene.
const SKIPPED: &[&str] = &[
    "ProtoDeclare",
    "ExternProtoDeclare",
    "ProtoInstance",
    "Script",
    "ROUTE",
    "IMPORT",
    "EXPORT",
    "field",
    "fieldValue",
    "IS",
    "connect",
    "meta",
    "component",
];

struct Open {
    node: Node,
    /// The field its parent holds it in.
    container: String,
    /// A USE: the node it stands for.
    used: Option<Rc<Node>>,
}

pub fn read(bytes: &[u8], progress: &mut Progress) -> Result<Loaded, String> {
    let unpacked;
    let bytes = if bytes.starts_with(&[0x1f, 0x8b]) {
        unpacked = super::xml::gunzip(bytes)?;
        &unpacked[..]
    } else {
        bytes
    };
    let text = utf8(bytes, "X3D data")?;
    let total = text.len().max(1);
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<Open> = Vec::new();
    let mut roots: Vec<Rc<Node>> = Vec::new();
    let mut defs: HashMap<String, Rc<Node>> = HashMap::new();
    let mut unit = None;
    let mut seen_x3d = false;
    let mut in_scene = false;
    // Depth inside an element whose content is skipped.
    let mut skipping = 0usize;
    let mut instances: Vec<String> = Vec::new();
    let mut reported = 0;
    loop {
        let event = reader.read_event().map_err(|e| xml_error("X3D data", &e))?;
        let at = reader.buffer_position() as usize;
        if at - reported >= 1 << 20 {
            reported = at;
            progress.report(Stage::Parse, at as f64 / total as f64);
        }
        let (e, empty) = match &event {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(_) => {
                if skipping > 0 {
                    skipping -= 1;
                } else if let Some(open) = stack.pop() {
                    finish(open, &mut stack, &mut roots, &mut defs);
                } else if in_scene {
                    in_scene = false;
                }
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        if skipping > 0 {
            if !empty {
                skipping += 1;
            }
            continue;
        }
        let name = local(e);
        match name.as_str() {
            "X3D" => {
                seen_x3d = true;
                continue;
            }
            "unit" => {
                if attribute(e, "category").as_deref() == Some("length")
                    && let Some(f) = attribute(e, "conversionFactor")
                        .and_then(|f| f.trim().parse::<f64>().ok())
                        .filter(|f| f.is_finite() && *f > 0.0)
                {
                    unit = Some(f * 1000.0);
                }
                continue;
            }
            "Scene" => {
                in_scene = !empty;
                continue;
            }
            "head" => continue,
            _ => {}
        }
        if SKIPPED.contains(&name.as_str()) || !in_scene {
            if name == "ProtoInstance" {
                let kind = attribute(e, "name").unwrap_or_else(|| "?".into());
                if !instances.contains(&kind) {
                    instances.push(kind);
                }
            }
            if !empty && in_scene {
                skipping = 1;
            }
            continue;
        }
        let open = element(e, &name, &defs);
        if empty {
            finish(open, &mut stack, &mut roots, &mut defs);
        } else {
            stack.push(open);
        }
    }
    if !seen_x3d {
        return Err("it is not an X3D file (it has no <X3D> element)".into());
    }
    let mut warnings = Vec::new();
    if !instances.is_empty() {
        warnings.push(format!(
            "Shapes made by the file's own node types ({}) were left out.",
            instances.join(", ")
        ));
    }
    super::vrml::from_scene(&roots, unit.unwrap_or(1.0), unit, warnings, progress)
}

fn local(e: &BytesStart) -> String {
    let name = e.local_name();
    let name: &str = name.as_ref();
    name.to_string()
}

/// An element as a node: its attributes as fields, or the node a USE names.
fn element(e: &BytesStart, kind: &str, defs: &HashMap<String, Rc<Node>>) -> Open {
    let mut node = Node::new(kind);
    let mut container = None;
    let mut used = None;
    for a in e.attributes().flatten() {
        let local = a.key.local_name();
        let key: &str = local.as_ref();
        let key = key.to_string();
        let Some(value) = attribute(e, &key) else {
            continue;
        };
        match key.as_str() {
            "DEF" => node.name = Some(value.trim().to_string()),
            "USE" => used = defs.get(value.trim()).cloned(),
            "containerField" => container = Some(value.trim().to_string()),
            _ => node.fields.push((key, field(&value))),
        }
    }
    Open {
        node,
        container: container.unwrap_or_else(|| "children".into()),
        used,
    }
}

/// An attribute's value: true or false, a list of numbers, or text.
fn field(value: &str) -> Field {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case("true") {
        return Field::Bool(true);
    }
    if trimmed.eq_ignore_ascii_case("false") {
        return Field::Bool(false);
    }
    let numbers: Result<Vec<f64>, _> = trimmed
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty())
        .map(str::parse::<f64>)
        .collect();
    match numbers {
        Ok(v) if !v.is_empty() => Field::Numbers(v),
        _ => Field::Text(vec![trimmed.to_string()]),
    }
}

/// Files a finished element under its parent (or as a root), and remembers its DEF name.
fn finish(
    open: Open,
    stack: &mut [Open],
    roots: &mut Vec<Rc<Node>>,
    defs: &mut HashMap<String, Rc<Node>>,
) {
    let node = match open.used {
        Some(n) => n,
        None => {
            let n = Rc::new(open.node);
            if let Some(name) = &n.name {
                defs.insert(name.clone(), n.clone());
            }
            n
        }
    };
    let Some(parent) = stack.last_mut() else {
        roots.push(node);
        return;
    };
    let fields = &mut parent.node.fields;
    if let Some((_, Field::Nodes(list))) = fields.iter_mut().find(|(n, _)| *n == open.container) {
        list.push(node);
    } else {
        fields.push((open.container, Field::Nodes(vec![node])));
    }
}

/// X3D 3.3 in millimetres: one Shape per part, its triangles in an IndexedTriangleSet.
pub fn write<W: Write>(mut w: W, parts: &[NamedMesh]) -> io::Result<W> {
    writeln!(w, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>")?;
    writeln!(
        w,
        "<!DOCTYPE X3D PUBLIC \"ISO//Web3D//DTD X3D 3.3//EN\" \"https://www.web3d.org/specifications/x3d-3.3.dtd\">"
    )?;
    writeln!(w, "<X3D profile=\"Interchange\" version=\"3.3\">")?;
    writeln!(w, "  <head>")?;
    writeln!(
        w,
        "    <unit category=\"length\" name=\"millimetres\" conversionFactor=\"0.001\"/>"
    )?;
    writeln!(w, "    <meta name=\"generator\" content=\"tricky3D\"/>")?;
    writeln!(w, "  </head>")?;
    writeln!(w, "  <Scene>")?;
    let mut taken = std::collections::HashSet::new();
    for p in parts {
        let name = super::vrml::def_name(p.name, &mut taken);
        writeln!(w, "    <Shape DEF=\"{}\">", escape(&name))?;
        writeln!(w, "      <Appearance><Material/></Appearance>")?;
        write!(w, "      <IndexedTriangleSet solid=\"true\" index=\"")?;
        for (k, t) in p.mesh.indices.as_chunks::<3>().0.iter().enumerate() {
            if k > 0 {
                write!(w, " ")?;
            }
            write!(w, "{} {} {}", t[0], t[1], t[2])?;
        }
        writeln!(w, "\">")?;
        write!(w, "        <Coordinate point=\"")?;
        for (k, v) in p.mesh.positions.as_chunks::<3>().0.iter().enumerate() {
            if k > 0 {
                write!(w, ", ")?;
            }
            write!(w, "{} {} {}", v[0], v[1], v[2])?;
        }
        writeln!(w, "\"/>")?;
        writeln!(w, "      </IndexedTriangleSet>")?;
        writeln!(w, "    </Shape>")?;
    }
    writeln!(w, "  </Scene>")?;
    writeln!(w, "</X3D>")?;
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{bounds, stats};

    fn read_text(text: &str) -> Result<Loaded, String> {
        read(text.as_bytes(), &mut Progress::silent())
    }

    pub(crate) const CUBE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<X3D profile="Interchange" version="3.3">
  <head><unit category="length" name="centimetres" conversionFactor="0.01"/></head>
  <Scene>
    <Transform translation="1 0 0">
      <Shape DEF="Cube">
        <Appearance><Material diffuseColor="0 0 1"/></Appearance>
        <IndexedFaceSet coordIndex="0 3 2 1 -1 4 5 6 7 -1 0 1 5 4 -1 1 2 6 5 -1 2 3 7 6 -1 3 0 4 7 -1">
          <Coordinate DEF="Pts" point="0 0 0, 1 0 0, 1 1 0, 0 1 0, 0 0 1, 1 0 1, 1 1 1, 0 1 1"/>
        </IndexedFaceSet>
      </Shape>
    </Transform>
    <Transform translation="0 0 3"><Shape USE="Cube"/></Transform>
    <ProtoInstance name="Gear"/>
    <ROUTE fromNode="a" fromField="b" toNode="c" toField="d"/>
  </Scene>
</X3D>"#;

    #[test]
    fn reads_a_cube_in_centimetres_with_use_and_colour() {
        let loaded = read_text(CUBE).unwrap();
        assert_eq!(loaded.unit, Some(10.0));
        assert_eq!(loaded.objects.len(), 1);
        let o = &loaded.objects[0];
        assert_eq!(o.name.as_deref(), Some("Cube"));
        assert_eq!(o.mesh.triangle_count(), 24);
        assert!((stats(&o.mesh).volume - 2000.0).abs() < 1e-2);
        assert_eq!(
            bounds(&o.mesh).unwrap(),
            [[0.0, 0.0, 0.0], [20.0, 10.0, 40.0]]
        );
        assert_eq!(o.color, Some([0, 0, 255]));
        assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
    }

    #[test]
    fn triangle_sets_with_vertex_colours() {
        let text = r#"<X3D><Scene><Shape><IndexedTriangleSet index="0 1 2 0 2 3" colorPerVertex="true">
            <Coordinate point="0 0 0 1 0 0 1 1 0 0 1 0"/><Color color="1 0 0 1 0 0 1 0 0 0 0 1"/>
          </IndexedTriangleSet></Shape></Scene></X3D>"#;
        let loaded = read_text(text).unwrap();
        let o = &loaded.objects[0];
        assert_eq!(o.mesh.triangle_count(), 2);
        assert_eq!(loaded.unit, None);
        assert_eq!(o.colors.as_deref(), Some(&[255, 0, 0, 170, 0, 85][..]));
    }

    #[test]
    fn errors_name_what_is_wrong() {
        assert_eq!(
            read_text("<a/>").unwrap_err(),
            "it is not an X3D file (it has no <X3D> element)"
        );
        assert!(read_text("<X3D><Scene><Shape></Scene></X3D>").is_err());
    }

    #[test]
    fn writes_what_it_reads_in_millimetres() {
        let loaded = read_text(CUBE).unwrap();
        let mesh = &loaded.objects[0].mesh;
        let parts = [NamedMesh { name: "Cube", mesh }];
        let bytes = write(Vec::new(), &parts).unwrap();
        let back = read(&bytes, &mut Progress::silent()).unwrap();
        assert_eq!(back.unit, Some(1.0));
        assert_eq!(back.objects[0].mesh, *mesh);
    }
}
