//! PLY (text, binary little-endian and big-endian) through ply-rs. Vertex x, y, z may be any
//! numeric type; faces come from a `vertex_indices` or `vertex_index` list of any integer type and
//! polygons are split into a fan of triangles. Every other element is read and ignored.

use std::io::{self, Write};

use ply_rs::parser::Parser;
use ply_rs::ply::{ElementDef, Encoding, Property, PropertyAccess, PropertyType, ScalarType};

use super::{Loaded, NamedMesh};
use crate::mesh::{fmt_count, weld_indexed};
use crate::progress::{Progress, Stage};

#[derive(Default)]
struct Vertex {
    p: [f32; 3],
}

impl PropertyAccess for Vertex {
    fn new() -> Self {
        Vertex::default()
    }

    fn set_property(&mut self, name: String, property: Property) {
        let axis = match name.as_str() {
            "x" => 0,
            "y" => 1,
            "z" => 2,
            _ => return,
        };
        if let Some(v) = scalar(&property) {
            self.p[axis] = v;
        }
    }
}

fn scalar(property: &Property) -> Option<f32> {
    Some(match *property {
        Property::Char(v) => v as f32,
        Property::UChar(v) => v as f32,
        Property::Short(v) => v as f32,
        Property::UShort(v) => v as f32,
        Property::Int(v) => v as f32,
        Property::UInt(v) => v as f32,
        Property::Float(v) => v,
        Property::Double(v) => v as f32,
        _ => return None,
    })
}

#[derive(Default)]
struct Face {
    corners: Option<Property>,
}

impl PropertyAccess for Face {
    fn new() -> Self {
        Face::default()
    }

    fn set_property(&mut self, name: String, property: Property) {
        if name == "vertex_indices" || name == "vertex_index" {
            self.corners = Some(property);
        }
    }
}

/// For elements tricky3D does not use.
struct Skip;

impl PropertyAccess for Skip {
    fn new() -> Self {
        Skip
    }
}

/// Copies an integer list property into `out`; false for anything else.
fn corner_list(property: &Property, out: &mut Vec<i64>) -> bool {
    out.clear();
    match property {
        Property::ListChar(v) => out.extend(v.iter().map(|&i| i as i64)),
        Property::ListUChar(v) => out.extend(v.iter().map(|&i| i as i64)),
        Property::ListShort(v) => out.extend(v.iter().map(|&i| i as i64)),
        Property::ListUShort(v) => out.extend(v.iter().map(|&i| i as i64)),
        Property::ListInt(v) => out.extend(v.iter().map(|&i| i as i64)),
        Property::ListUInt(v) => out.extend(v.iter().map(|&i| i as i64)),
        _ => return false,
    }
    true
}

pub fn read(bytes: &[u8], progress: &mut Progress) -> Result<Loaded, String> {
    let header_end = check_header(bytes)?;
    // ply-rs rejects a header that is not UTF-8, which comments written in a Windows code page
    // are not; numbers and keywords are ASCII, so a lossy copy reads the same.
    let head = String::from_utf8_lossy(&bytes[..header_end]);
    let header = Parser::<Skip>::new()
        .read_header(&mut head.as_bytes())
        .map_err(|_| "its PLY header is not valid".to_string())?;
    let mut reader: &[u8] = &bytes[header_end..];

    let vertex = header.elements.get("vertex").ok_or("it has no vertices")?;
    for axis in ["x", "y", "z"] {
        match vertex.properties.get(axis).map(|p| &p.data_type) {
            Some(PropertyType::Scalar(_)) => {}
            _ => return Err(format!("its vertices have no {axis} coordinate")),
        }
    }
    let face = header
        .elements
        .get("face")
        .ok_or("it has no faces (point clouds cannot be opened)")?;
    let list_ok = ["vertex_indices", "vertex_index"].iter().any(|name| {
        matches!(face.properties.get(*name).map(|p| &p.data_type),
            Some(PropertyType::List(_, t)) if !matches!(t, ScalarType::Float | ScalarType::Double))
    });
    if !list_ok {
        return Err("its faces have no vertex index list".into());
    }

    let vertex_count = vertex.count;
    let total = header
        .elements
        .values()
        .fold(0usize, |sum, e| sum.saturating_add(e.count))
        .max(1);
    let mut done = 0usize;
    let mut positions: Vec<f32> = Vec::with_capacity(vertex_count.min(bytes.len() / 3) * 3);
    let mut indices: Vec<u32> = Vec::with_capacity(face.count.min(bytes.len() / 4) * 3);
    let mut corners = Vec::new();
    let mut skipped = 0usize;
    let mut face_number = 0usize;
    for (name, def) in &header.elements {
        let mut tick = |done: &mut usize| {
            *done += 1;
            if (*done).is_multiple_of(65_536) {
                progress.report(Stage::Parse, *done as f64 / total as f64);
            }
        };
        match name.as_str() {
            "vertex" => read_elements::<Vertex>(&mut reader, def, header.encoding, |v| {
                positions.extend_from_slice(&v.p);
                tick(&mut done);
                Ok(())
            })?,
            "face" => read_elements::<Face>(&mut reader, def, header.encoding, |f| {
                face_number += 1;
                tick(&mut done);
                let Some(list) = f.corners else { return Ok(()) };
                if !corner_list(&list, &mut corners) {
                    return Err("its faces have no vertex index list".into());
                }
                if corners.len() < 3 {
                    skipped += 1;
                    return Ok(());
                }
                if corners
                    .iter()
                    .any(|&i| i < 0 || i as u64 >= vertex_count as u64)
                {
                    return Err(format!(
                        "face {} refers to a vertex that does not exist",
                        fmt_count(face_number as u64)
                    ));
                }
                for k in 1..corners.len() - 1 {
                    indices.extend_from_slice(&[
                        corners[0] as u32,
                        corners[k] as u32,
                        corners[k + 1] as u32,
                    ]);
                }
                Ok(())
            })?,
            _ => read_elements::<Skip>(&mut reader, def, header.encoding, |_| {
                tick(&mut done);
                Ok(())
            })?,
        }
    }

    let mesh = weld_indexed(&positions, &indices)?;
    progress.report(Stage::Weld, 1.0);
    let mut loaded = Loaded::single(mesh);
    if skipped > 0 {
        loaded.warnings.push(format!(
            "{} faces with fewer than three corners were skipped.",
            fmt_count(skipped as u64)
        ));
    }
    Ok(loaded)
}

/// Reads `def.count` elements of one kind, one at a time so nothing big is buffered.
fn read_elements<E: PropertyAccess>(
    reader: &mut &[u8],
    def: &ElementDef,
    encoding: Encoding,
    mut each: impl FnMut(E) -> Result<(), String>,
) -> Result<(), String> {
    // An element without properties holds no data: in binary it takes no bytes, so a huge count
    // would loop for years without ever reaching the end of the file.
    if def.properties.is_empty() {
        return Ok(());
    }
    let parser = Parser::<E>::new();
    let kind = if def.name == "vertex" {
        "vertices"
    } else if def.name == "face" {
        "faces"
    } else {
        "elements"
    };
    let ends_early = |found: usize| {
        format!(
            "the file ends early (expected {} {kind}, found {})",
            fmt_count(def.count as u64),
            fmt_count(found as u64)
        )
    };
    for i in 0..def.count {
        let element = match encoding {
            Encoding::Ascii => {
                let Some(line) = next_line(reader) else {
                    return Err(ends_early(i));
                };
                let line = std::str::from_utf8(line)
                    .map_err(|_| format!("its PLY text is not valid at {} {}", def.name, i + 1))?;
                // ply-rs only knows a lower-case exponent marker.
                let line = if line.contains('E') {
                    line.to_ascii_lowercase().into()
                } else {
                    std::borrow::Cow::Borrowed(line)
                };
                parser.read_ascii_element(&line, def).or_else(|e| {
                    // ply-rs takes only digits, so "nan" or "-nan(ind)" in the normals of a
                    // degenerate face fails the line; those values are not used anyway.
                    zero_unused(&line, def)
                        .map_or(Err(e), |line| parser.read_ascii_element(&line, def))
                })
            }
            Encoding::BinaryLittleEndian => parser.read_little_endian_element(reader, def),
            Encoding::BinaryBigEndian => parser.read_big_endian_element(reader, def),
        };
        let element = match element {
            Ok(e) => e,
            Err(_) if !matches!(encoding, Encoding::Ascii) && reader.is_empty() => {
                return Err(ends_early(i));
            }
            Err(_) => {
                return Err(format!(
                    "its PLY data is not valid at {} {}",
                    def.name,
                    fmt_count(i as u64 + 1)
                ));
            }
        };
        each(element)?;
    }
    Ok(())
}

/// A text element line with every single value tricky3D does not read (anything but x, y, z and
/// the face corners) set to 0; None when the line does not split into the element's properties.
fn zero_unused(line: &str, def: &ElementDef) -> Option<String> {
    let mut values = line.split_whitespace();
    let mut out: Vec<&str> = Vec::new();
    for (name, p) in &def.properties {
        let used = matches!(
            name.as_str(),
            "x" | "y" | "z" | "vertex_indices" | "vertex_index"
        );
        match p.data_type {
            PropertyType::Scalar(_) => {
                let value = values.next()?;
                out.push(if used { value } else { "0" });
            }
            PropertyType::List(..) => {
                let count = values.next()?;
                out.push(count);
                for _ in 0..count.parse::<usize>().ok()? {
                    out.push(values.next()?);
                }
            }
        }
    }
    Some(out.join(" "))
}

/// The next non-blank line of a text payload, without its line break.
fn next_line<'a>(reader: &mut &'a [u8]) -> Option<&'a [u8]> {
    loop {
        if reader.is_empty() {
            return None;
        }
        let end = reader
            .iter()
            .position(|&b| b == b'\n')
            .unwrap_or(reader.len());
        let line = &reader[..end];
        *reader = &reader[(end + 1).min(reader.len())..];
        if line.iter().any(|b| !b.is_ascii_whitespace()) {
            return Some(line);
        }
    }
}

/// The length of the header, through its `end_header` line.
///
/// ply-rs panics on a header number too big for u64 (the release build aborts on a panic). Its
/// grammar reads numbers only on `format` and `element` lines, so those are checked on every
/// line ply-rs reads, up to the line it ends the header at.
fn check_header(bytes: &[u8]) -> Result<usize, String> {
    if !bytes.starts_with(b"ply") {
        return Err("it is not a PLY file".into());
    }
    // ply-rs reads the header with read_line, so lines end at '\n' only.
    let mut read = 0;
    for line in bytes.split(|&b| b == b'\n') {
        read += line.len() + 1;
        if ends_header(line) {
            return Ok(read.min(bytes.len()));
        }
        let start = line
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(line.len());
        let line = &line[start..];
        if line.starts_with(b"element") || line.starts_with(b"format") {
            let mut run = 0;
            for &b in line {
                run = if b.is_ascii_digit() { run + 1 } else { 0 };
                if run > 18 {
                    return Err("its PLY header has a number that is too large".into());
                }
            }
        }
    }
    Err("its PLY header is incomplete".into())
}

/// The line ply-rs's grammar takes as the end of the header: `end_header`, then optional spaces
/// or tabs, then an optional carriage return, and nothing else. Text such as
/// `comment end_header` does not end it.
fn ends_header(line: &[u8]) -> bool {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let end = line
        .iter()
        .rposition(|&b| b != b' ' && b != b'\t')
        .map_or(0, |i| i + 1);
    &line[..end] == b"end_header"
}

/// Binary little-endian PLY with all parts merged into one mesh.
pub fn write<W: Write>(mut w: W, parts: &[NamedMesh]) -> io::Result<W> {
    let vertices: usize = parts.iter().map(|p| p.mesh.vertex_count()).sum();
    let faces: usize = parts.iter().map(|p| p.mesh.triangle_count()).sum();
    if i32::try_from(vertices).is_err() {
        return Err(io::Error::other("PLY cannot hold this many vertices"));
    }
    write!(
        w,
        "ply\nformat binary_little_endian 1.0\ncomment tricky3D\nelement vertex {vertices}\nproperty float x\nproperty float y\nproperty float z\nelement face {faces}\nproperty list uchar int vertex_indices\nend_header\n"
    )?;
    for p in parts {
        for v in &p.mesh.positions {
            w.write_all(&v.to_le_bytes())?;
        }
    }
    let mut base = 0i32;
    let mut record = [0u8; 13];
    record[0] = 3;
    for p in parts {
        for t in p.mesh.indices.as_chunks::<3>().0 {
            for (k, &i) in t.iter().enumerate() {
                record[1 + k * 4..5 + k * 4].copy_from_slice(&(base + i as i32).to_le_bytes());
            }
            w.write_all(&record)?;
        }
        base += p.mesh.vertex_count() as i32;
    }
    Ok(w)
}

/// Text PLY, for sample files and tests.
pub fn write_text<W: Write>(mut w: W, parts: &[NamedMesh]) -> io::Result<W> {
    let vertices: usize = parts.iter().map(|p| p.mesh.vertex_count()).sum();
    let faces: usize = parts.iter().map(|p| p.mesh.triangle_count()).sum();
    write!(
        w,
        "ply\nformat ascii 1.0\ncomment tricky3D\nelement vertex {vertices}\nproperty float x\nproperty float y\nproperty float z\nelement face {faces}\nproperty list uchar int vertex_indices\nend_header\n"
    )?;
    for p in parts {
        for v in p.mesh.positions.as_chunks::<3>().0 {
            writeln!(w, "{} {} {}", v[0], v[1], v[2])?;
        }
    }
    let mut base = 0u64;
    for p in parts {
        for t in p.mesh.indices.as_chunks::<3>().0 {
            writeln!(
                w,
                "3 {} {} {}",
                base + t[0] as u64,
                base + t[1] as u64,
                base + t[2] as u64
            )?;
        }
        base += p.mesh.vertex_count() as u64;
    }
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_normals_are_ignored_but_positions_are_not() {
        let file = |first: &str| {
            format!(
                "ply
format ascii 1.0
element vertex 3
property float x
property float y
property float z
property float nx
property float ny
property float nz
element face 1
property list uchar int vertex_indices
property float quality
end_header
{first}
1 0 0 0 0 1
0 1 0 0 0 1
3 0 1 2 -nan(ind)
"
            )
        };
        let loaded = read(
            file("0 0 0 -nan(ind) nan 1.#QNAN0").as_bytes(),
            &mut Progress::silent(),
        )
        .unwrap();
        assert_eq!(loaded.objects[0].mesh.triangle_count(), 1);
        assert_eq!(
            read(file("0 nan 0 0 0 1").as_bytes(), &mut Progress::silent()).unwrap_err(),
            "its PLY data is not valid at vertex 1"
        );
    }

    #[test]
    fn huge_header_numbers_are_an_error_not_a_crash() {
        let file = b"ply\nformat ascii 1.0\nelement vertex 99999999999999999999999\nproperty float x\nend_header\n";
        assert_eq!(
            read(file, &mut Progress::silent()).unwrap_err(),
            "its PLY header has a number that is too large"
        );
    }

    #[test]
    fn end_header_text_before_the_real_end_cannot_hide_a_huge_number() {
        // ply-rs ends the header only at a line that is exactly "end_header"; these lines are
        // a comment, obj_info and a property name it reads past.
        for early in [
            "comment end_header",
            "obj_info end_header",
            "property float end_header",
        ] {
            let file = format!(
                "ply\nformat ascii 1.0\nelement vertex 1\n{early}\nelement face 99999999999999999999999\nproperty list uchar int vertex_indices\nend_header\n"
            );
            assert_eq!(
                read(file.as_bytes(), &mut Progress::silent()).unwrap_err(),
                "its PLY header has a number that is too large",
                "{early}"
            );
        }
    }

    #[test]
    fn comments_with_end_header_or_a_code_page_name_still_load() {
        let file = b"ply\nformat ascii 1.0\ncomment end_header\nobj_info W\xFCrfel\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar int vertex_indices\nend_header \r\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n";
        let loaded = read(file, &mut Progress::silent()).unwrap();
        assert_eq!(loaded.objects[0].mesh.triangle_count(), 1);
    }

    #[test]
    fn elements_without_properties_are_skipped_at_once() {
        // In binary such an element takes no bytes, so a huge count must not mean a huge loop.
        let mut file = b"ply\nformat binary_little_endian 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement junk 999999999999999999\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n".to_vec();
        for v in [0f32, 0., 0., 1., 0., 0., 0., 1., 0.] {
            file.extend_from_slice(&v.to_le_bytes());
        }
        file.push(3);
        for i in [0i32, 1, 2] {
            file.extend_from_slice(&i.to_le_bytes());
        }
        let started = std::time::Instant::now();
        let loaded = read(&file, &mut Progress::silent()).unwrap();
        assert_eq!(loaded.objects[0].mesh.triangle_count(), 1);
        assert!(started.elapsed().as_secs() < 1);

        let text = b"ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement junk 999999999999999999\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n";
        let loaded = read(text, &mut Progress::silent()).unwrap();
        assert_eq!(loaded.objects[0].mesh.triangle_count(), 1);
    }

    #[test]
    fn missing_faces_and_short_files() {
        let points = b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nend_header\n0 0 0\n";
        assert!(
            read(points, &mut Progress::silent())
                .unwrap_err()
                .contains("no faces")
        );
        let short = b"ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0\n1 0 0\n";
        assert_eq!(
            read(short, &mut Progress::silent()).unwrap_err(),
            "the file ends early (expected 3 vertices, found 2)"
        );
        let bad_index = b"ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0\n1 0 0\n0 1 0\n3 0 1 5\n";
        assert_eq!(
            read(bad_index, &mut Progress::silent()).unwrap_err(),
            "face 1 refers to a vertex that does not exist"
        );
    }

    #[test]
    fn double_coordinates_short_indices_and_extra_elements() {
        let file = b"ply\nformat ascii 1.0\nelement vertex 4\nproperty double x\nproperty double y\nproperty double z\nproperty uchar red\nelement face 1\nproperty list uchar ushort vertex_index\nelement edge 1\nproperty int vertex1\nproperty int vertex2\nend_header\n0 0 0 255\n1 0 0 0\n1 1 0 0\n0 1 0 0\n4 0 1 2 3\n0 1\n";
        let loaded = read(file, &mut Progress::silent()).unwrap();
        assert_eq!(loaded.objects[0].mesh.triangle_count(), 2);
        assert_eq!(loaded.objects[0].mesh.vertex_count(), 4);
    }
}
