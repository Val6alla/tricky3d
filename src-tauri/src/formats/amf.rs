//! AMF (Additive Manufacturing File Format), plain XML or zipped. Each object becomes one part
//! with all its volumes in one mesh, named from `<metadata type="name">`; the `unit` attribute is
//! converted to millimetres. Constellations (arrangements of objects) are not applied.

use std::io::{self, Write};

use quick_xml::events::Event;
use quick_xml::reader::Reader;

use super::xml::{
    Budget, append_text, attribute, entries_ending, index, number, open_zip, read_entry,
    unit_scale, utf8, xml_error,
};
use super::{Loaded, NamedMesh, Object, clean_name};
use crate::mesh::{fmt_count, weld_indexed};
use crate::progress::{Progress, Stage};

pub fn read(bytes: &[u8], progress: &mut Progress) -> Result<Loaded, String> {
    if bytes.starts_with(b"PK\x03\x04") {
        let mut zip = open_zip(bytes)?;
        let files = entries_ending(&mut zip, "");
        let name = files
            .iter()
            .find(|(n, _)| n.to_ascii_lowercase().ends_with(".amf"))
            .or(files.first())
            .map(|(n, _)| n.clone())
            .ok_or("its zip package is empty")?;
        let data = read_entry(&mut zip, &name)?;
        return parse(utf8(&data, "AMF data")?, &mut Budget::default(), progress);
    }
    parse(utf8(bytes, "AMF data")?, &mut Budget::default(), progress)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tag {
    Amf,
    Object,
    ObjectMetadata,
    Mesh,
    Vertices,
    Vertex,
    Coordinates,
    Coordinate(usize),
    Volume,
    Triangle,
    Corner(usize),
    Constellation,
    Other,
}

fn tag_for(parent: Option<Tag>, local: &str) -> Tag {
    match (parent, local) {
        (None, "amf") => Tag::Amf,
        (Some(Tag::Amf), "object") => Tag::Object,
        (Some(Tag::Amf), "constellation") => Tag::Constellation,
        (Some(Tag::Object), "metadata") => Tag::ObjectMetadata,
        (Some(Tag::Object), "mesh") => Tag::Mesh,
        (Some(Tag::Mesh), "vertices") => Tag::Vertices,
        (Some(Tag::Vertices), "vertex") => Tag::Vertex,
        (Some(Tag::Vertex), "coordinates") => Tag::Coordinates,
        (Some(Tag::Coordinates), "x") => Tag::Coordinate(0),
        (Some(Tag::Coordinates), "y") => Tag::Coordinate(1),
        (Some(Tag::Coordinates), "z") => Tag::Coordinate(2),
        (Some(Tag::Mesh), "volume") => Tag::Volume,
        (Some(Tag::Volume), "triangle") => Tag::Triangle,
        (Some(Tag::Triangle), "v1") => Tag::Corner(0),
        (Some(Tag::Triangle), "v2") => Tag::Corner(1),
        (Some(Tag::Triangle), "v3") => Tag::Corner(2),
        _ => Tag::Other,
    }
}

#[derive(Default)]
struct AmfObject {
    name: Option<String>,
    positions: Vec<f64>,
    indices: Vec<u32>,
}

/// Parses AMF text, counting vertices and triangles against the limits as they are read.
fn parse(text: &str, budget: &mut Budget, progress: &mut Progress) -> Result<Loaded, String> {
    let bad = |what: String| format!("its AMF data has {what}");
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<Tag> = Vec::new();
    let mut objects: Vec<AmfObject> = Vec::new();
    let mut object = AmfObject::default();
    let mut mesh_base = 0usize;
    let mut point = [None; 3];
    let mut corners = [None; 3];
    let mut text_buf = String::new();
    let mut metadata_is_name = false;
    let mut unit = None;
    let mut constellations = false;
    let mut elements = 0usize;
    loop {
        let event = reader.read_event().map_err(|e| xml_error("AMF data", &e))?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                let tag = tag_for(stack.last().copied(), e.local_name().as_ref());
                let is_start = matches!(event, Event::Start(_));
                match tag {
                    Tag::Amf => unit = attribute(e, "unit"),
                    Tag::Object => object = AmfObject::default(),
                    Tag::ObjectMetadata => {
                        metadata_is_name = attribute(e, "type").is_some_and(|t| t.trim() == "name")
                    }
                    Tag::Mesh => mesh_base = object.positions.len() / 3,
                    Tag::Vertex => {
                        budget.add(1, 0, "has")?;
                        point = [None; 3];
                    }
                    Tag::Triangle => {
                        budget.add(0, 1, "has")?;
                        corners = [None; 3];
                    }
                    Tag::Constellation => constellations = true,
                    Tag::Other if is_start => {
                        reader
                            .read_to_end(e.name())
                            .map_err(|e| xml_error("AMF data", &e))?;
                        continue;
                    }
                    _ => {}
                }
                text_buf.clear();
                if is_start {
                    stack.push(tag);
                } else if tag != Tag::Other {
                    end_tag(
                        tag,
                        &mut object,
                        &mut objects,
                        mesh_base,
                        &point,
                        &corners,
                        "",
                        metadata_is_name,
                    )
                    .map_err(bad)?;
                }
                elements += 1;
                if elements.is_multiple_of(65_536) {
                    progress.report(
                        Stage::Parse,
                        reader.buffer_position() as f64 / text.len().max(1) as f64,
                    );
                }
            }
            Event::End(_) => {
                let Some(tag) = stack.pop() else { continue };
                match tag {
                    Tag::Coordinate(axis) => {
                        point[axis] = Some(
                            number(&text_buf)
                                .ok_or_else(|| bad("a coordinate that is not a number".into()))?,
                        );
                    }
                    Tag::Corner(k) => {
                        corners[k] = Some(index(&text_buf).ok_or_else(|| {
                            bad("a triangle corner that is not a vertex number".into())
                        })?);
                    }
                    _ => end_tag(
                        tag,
                        &mut object,
                        &mut objects,
                        mesh_base,
                        &point,
                        &corners,
                        &text_buf,
                        metadata_is_name,
                    )
                    .map_err(bad)?,
                }
                text_buf.clear();
            }
            Event::Text(_) | Event::CData(_) | Event::GeneralRef(_) => {
                append_text(&mut text_buf, &event)
            }
            Event::Eof => break,
            _ => {}
        }
    }

    let mut warnings = Vec::new();
    let scale = match unit.as_deref() {
        None => 1.0,
        Some(u) => unit_scale(u).unwrap_or_else(|| {
            warnings.push(format!(
                "The unit \"{u}\" is unknown, so the sizes were read as millimetres."
            ));
            1.0
        }),
    };
    if constellations {
        warnings.push("The file arranges its objects in constellations; each object was loaded on its own instead.".into());
    }
    let object_count = objects.len();
    let mut out = Vec::with_capacity(objects.len());
    for (n, o) in objects.into_iter().enumerate() {
        let positions: Vec<f32> = o.positions.iter().map(|v| (v * scale) as f32).collect();
        let mesh = weld_indexed(&positions, &o.indices)?;
        out.push(Object {
            name: o.name,
            matrix: None,
            mesh,
        });
        progress.report(Stage::Weld, (n + 1) as f64 / object_count as f64);
    }
    Ok(Loaded {
        objects: out,
        object_count,
        splittable: false,
        warnings,
    })
}

#[allow(clippy::too_many_arguments)]
fn end_tag(
    tag: Tag,
    object: &mut AmfObject,
    objects: &mut Vec<AmfObject>,
    mesh_base: usize,
    point: &[Option<f64>; 3],
    corners: &[Option<u32>; 3],
    text: &str,
    metadata_is_name: bool,
) -> Result<(), String> {
    match tag {
        Tag::ObjectMetadata if metadata_is_name => object.name = clean_name(Some(text)),
        Tag::Vertex => {
            let [Some(x), Some(y), Some(z)] = *point else {
                return Err(format!(
                    "a vertex without x, y and z (vertex {})",
                    fmt_count(object.positions.len() as u64 / 3 + 1)
                ));
            };
            object.positions.extend_from_slice(&[x, y, z]);
        }
        Tag::Triangle => {
            let [Some(a), Some(b), Some(c)] = *corners else {
                return Err("a triangle without v1, v2 and v3".into());
            };
            let count = object.positions.len() / 3 - mesh_base;
            if [a, b, c].iter().any(|&i| i as usize >= count) {
                return Err("a triangle that refers to a vertex that does not exist".into());
            }
            let base = mesh_base as u32;
            object
                .indices
                .extend_from_slice(&[base + a, base + b, base + c]);
        }
        Tag::Object => objects.push(std::mem::take(object)),
        _ => {}
    }
    Ok(())
}

/// Plain AMF, unit millimetre, one object per part.
pub fn write<W: Write>(mut w: W, parts: &[NamedMesh]) -> io::Result<W> {
    use super::xml::escape;
    writeln!(
        w,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<amf unit=\"millimeter\" version=\"1.1\">"
    )?;
    writeln!(
        w,
        " <metadata type=\"producer\">{}</metadata>",
        escape(&super::threemf::application())
    )?;
    for (k, p) in parts.iter().enumerate() {
        writeln!(
            w,
            " <object id=\"{k}\">\n  <metadata type=\"name\">{}</metadata>\n  <mesh>\n   <vertices>",
            escape(p.name)
        )?;
        for v in p.mesh.positions.as_chunks::<3>().0 {
            writeln!(
                w,
                "    <vertex><coordinates><x>{}</x><y>{}</y><z>{}</z></coordinates></vertex>",
                v[0], v[1], v[2]
            )?;
        }
        writeln!(w, "   </vertices>\n   <volume>")?;
        for t in p.mesh.indices.as_chunks::<3>().0 {
            writeln!(
                w,
                "    <triangle><v1>{}</v1><v2>{}</v2><v3>{}</v3></triangle>",
                t[0], t[1], t[2]
            )?;
        }
        writeln!(w, "   </volume>\n  </mesh>\n </object>")?;
    }
    writeln!(w, "</amf>")?;
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::xml::{MAX_TRIANGLES, MAX_VERTICES};

    fn amf(vertices: usize, triangles: usize) -> String {
        format!(
            "<amf><object id=\"0\"><mesh><vertices>{}</vertices><volume>{}</volume></mesh></object></amf>",
            "<vertex><coordinates><x>0</x><y>0</y><z>0</z></coordinates></vertex>".repeat(vertices),
            "<triangle><v1>0</v1><v2>1</v2><v3>2</v3></triangle>".repeat(triangles),
        )
    }

    #[test]
    fn limits_stop_reading_at_the_element_that_crosses_them() {
        let mut budget = Budget {
            vertices: MAX_VERTICES - 2,
            ..Budget::default()
        };
        let err = parse(&amf(3, 1), &mut budget, &mut Progress::silent()).unwrap_err();
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
        let err = parse(&amf(3, 2), &mut budget, &mut Progress::silent()).unwrap_err();
        assert_eq!(
            err,
            "it has more than 50,000,000 triangles, which is more than tricky3D can handle"
        );

        let mut budget = Budget::default();
        let loaded = parse(&amf(3, 2), &mut budget, &mut Progress::silent()).unwrap();
        assert_eq!(loaded.objects[0].mesh.triangle_count(), 2);
        assert_eq!((budget.vertices, budget.triangles), (3, 2));
    }
}
