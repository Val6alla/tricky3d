//! STL, binary and text. stl_io parses the triangles; detection and welding are ours, because
//! stl_io decides binary vs text by the "solid" prefix alone, which many binary files also have.

use std::fs::File;
use std::io::{self, BufReader, Cursor, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::Path;

use super::{Loaded, NamedMesh, read_all, read_error};
use crate::mesh::{Mesh, Welder, fmt_count, triangle_normal};
use crate::progress::{Progress, Stage};

/// What the first bytes and the size say about a file.
#[derive(Debug, PartialEq, Eq)]
pub enum Kind {
    /// Binary STL with this many triangles (the size matches exactly).
    Binary(u32),
    /// Binary STL whose header declares one triangle more than the file holds, a writer's
    /// off-by-one; it holds this many whole triangles.
    OneShort(u32),
    Text,
    /// A binary header whose triangle count needs more bytes than the file has.
    Truncated {
        expected: u32,
        found: u64,
    },
    /// A binary header whose triangle count does not explain the file size.
    SizeMismatch {
        declared: u32,
    },
    Empty,
    NotStl,
}

/// A binary header whose triangles would need more than this many times the bytes the file has,
/// and that claims more triangles than any real STL tricky3D is meant for, is not a cut-off STL but
/// some other file whose bytes at offset 80 make a random count. A real STL cut off early (a broken
/// download) keeps a plausible count and is reported as ending early.
const NOT_STL_FACTOR: u64 = 100;
const NOT_STL_MIN_COUNT: u32 = 50_000_000;

/// Binary when the size is exactly 84 + 50 × the count at offset 80, even if the header starts
/// with "solid". Otherwise text is STL text when it starts with "solid" (any case, after
/// whitespace) and some other file when it does not, such as a web page or a STEP file.
pub fn detect(head: &[u8], size: u64) -> Kind {
    if size == 0 {
        return Kind::Empty;
    }
    let declared = head
        .get(80..84)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]));
    let expected = declared.map(|n| 84 + 50 * n as u64);
    if let (Some(n), Some(expected)) = (declared, expected)
        && size == expected
    {
        return Kind::Binary(n);
    }
    if looks_like_text(head) {
        return if starts_with_solid(head) {
            Kind::Text
        } else {
            Kind::NotStl
        };
    }
    match (declared, expected) {
        (Some(n), Some(expected)) if n > NOT_STL_MIN_COUNT && expected / NOT_STL_FACTOR > size => {
            Kind::NotStl
        }
        (Some(n), Some(expected)) if n > 0 && size == expected - 50 => Kind::OneShort(n - 1),
        (Some(n), Some(expected)) if size < expected => Kind::Truncated {
            expected: n,
            found: (size - 84) / 50,
        },
        (Some(n), Some(_)) => Kind::SizeMismatch { declared: n },
        _ => Kind::NotStl,
    }
}

fn starts_with_solid(head: &[u8]) -> bool {
    let head = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
    is_solid(first_word(head))
}

/// "solid" in any case, also run into a name ("solidPart"), as writers do.
fn is_solid(word: &[u8]) -> bool {
    word.get(..5)
        .is_some_and(|w| w.eq_ignore_ascii_case(b"solid"))
}

/// The first whitespace-separated word of a line.
fn first_word(line: &[u8]) -> &[u8] {
    line.split(u8::is_ascii_whitespace)
        .find(|w| !w.is_empty())
        .unwrap_or_default()
}

/// Text STL has no control bytes; a binary header and triangle count almost always do.
fn looks_like_text(head: &[u8]) -> bool {
    head.iter()
        .all(|&b| b >= 0x20 || matches!(b, b'\t' | b'\n' | b'\r' | 0x0c))
}

pub fn read_path(path: &Path, progress: &mut Progress) -> Result<Loaded, String> {
    let mut file = File::open(path).map_err(|e| read_error(&e))?;
    let size = file.metadata().map_err(|e| read_error(&e))?.len();
    let mut head = [0u8; 84];
    let got = read_up_to(&mut file, &mut head).map_err(|e| read_error(&e))?;
    let mut warnings = Vec::new();
    let mesh = match detect(&head[..got], size) {
        Kind::Binary(n) => {
            file.seek(SeekFrom::Start(0)).map_err(|e| read_error(&e))?;
            read_binary(file, n, progress)?
        }
        Kind::OneShort(n) => {
            file.seek(SeekFrom::Start(0)).map_err(|e| read_error(&e))?;
            warnings.push(format!(
                "The file declares {} triangles but holds {}; those were read.",
                fmt_count(n as u64 + 1),
                fmt_count(n as u64)
            ));
            read_binary(file, n, progress)?
        }
        Kind::Text => {
            drop(file);
            parse_text(&read_all(path, progress)?, progress)?
        }
        Kind::Truncated { expected, found } => return Err(ends_early(expected, found)),
        Kind::SizeMismatch { declared } => {
            return Err(format!(
                "its size does not match the {} triangles it declares",
                fmt_count(declared as u64)
            ));
        }
        Kind::Empty => return Err("the file is empty".into()),
        Kind::NotStl => return Err("it is not an STL file".into()),
    };
    let mut loaded = Loaded::single(mesh);
    loaded.warnings = warnings;
    Ok(loaded)
}

fn ends_early(expected: u32, found: u64) -> String {
    format!(
        "the file ends early (expected {} triangles, found {})",
        fmt_count(expected as u64),
        fmt_count(found)
    )
}

fn read_up_to(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match file.read(&mut buf[got..])? {
            0 => break,
            n => got += n,
        }
    }
    Ok(got)
}

/// Streams a binary STL through stl_io's triangle iterator into the welder.
fn read_binary(file: File, count: u32, progress: &mut Progress) -> Result<Mesh, String> {
    let mut source = BinaryOnly {
        inner: BufReader::with_capacity(1 << 20, file),
        pos: 0,
    };
    let triangles = stl_io::create_stl_reader(&mut source).map_err(|e| read_error(&e))?;
    let mut welder = Welder::new(count as usize * 3);
    let mut done = 0u64;
    // `take`: the header may declare one triangle more than `count` (`Kind::OneShort`).
    for t in triangles.take(count as usize) {
        let t = t.map_err(|e| match e.kind() {
            io::ErrorKind::UnexpectedEof => ends_early(count, done),
            _ => read_error(&e),
        })?;
        welder.triangle(t.vertices[0].0, t.vertices[1].0, t.vertices[2].0);
        done += 1;
        if done.is_multiple_of(65_536) {
            progress.report(Stage::Parse, done as f64 / count as f64);
        }
    }
    if done < count as u64 {
        return Err(ends_early(count, done));
    }
    progress.report(Stage::Weld, 1.0);
    welder.finish()
}

/// Presents a binary STL to stl_io so its "solid" probe cannot mistake it for text: the first
/// byte reads as a line break (the 80-byte header is ignored anyway).
struct BinaryOnly<R> {
    inner: R,
    pos: u64,
}

impl<R: Read> Read for BinaryOnly<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if self.pos == 0 && n > 0 {
            buf[0] = b'\n';
        }
        self.pos += n as u64;
        Ok(n)
    }
}

impl<R: Seek> Seek for BinaryOnly<R> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.pos = self.inner.seek(to)?;
        Ok(self.pos)
    }
}

/// Parses STL text. A file with several `solid` blocks becomes one mesh. Each block goes to
/// stl_io with a fixed `solid` line (stl_io needs "solid " plus a name) and a fixed `endsolid`
/// line (some writers leave it out), so names in a Windows code page, which stl_io rejects as
/// text that is not UTF-8, never reach it.
pub fn parse_text(bytes: &[u8], progress: &mut Progress) -> Result<Mesh, String> {
    let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let bodies = solid_bodies(text);
    if bodies.is_empty() {
        return Err("it is not an STL file".into());
    }
    let mut welder = Welder::new(text.len() / 70);
    let mut done = 0u64;
    for body in bodies {
        let mut source = Vec::with_capacity(body.len() + 48);
        source.extend_from_slice(b"solid tricky3d\n");
        let facets = normalise_body(&text[body.clone()], &mut source);
        source.extend_from_slice(b"\nendsolid tricky3d\n");
        let invalid = |done: u64| {
            // Text that starts with "solid" but has no facet line at all is some other text.
            if done == 0 && facets == 0 {
                return "it is not an STL file".to_string();
            }
            format!(
                "its STL text is not valid near triangle {}",
                fmt_count(done + 1)
            )
        };
        let mut cursor = Cursor::new(source);
        let triangles = stl_io::create_stl_reader(&mut cursor).map_err(|_| invalid(done))?;
        let mut in_body = 0usize;
        for t in triangles {
            let t = t.map_err(|_| invalid(done))?;
            welder.triangle(t.vertices[0].0, t.vertices[1].0, t.vertices[2].0);
            done += 1;
            in_body += 1;
            if done.is_multiple_of(16_384) {
                // About 250 bytes per facet in typical STL text.
                let at = (body.start + in_body * 250).min(body.end);
                progress.report(Stage::Parse, at as f64 / text.len() as f64);
            }
        }
    }
    progress.report(Stage::Weld, 1.0);
    welder.finish()
}

/// Copies a solid's body for stl_io with its keywords in lower case (some writers use upper case)
/// and every facet line as "facet normal 0 0 0". Normals are recomputed anyway, and writers put
/// "nan", "-nan(ind)", "1.#QNAN0", "inf" or nothing there for degenerate triangles, which stl_io
/// would reject. Line for line, so errors still name the right triangle. Returns the facet count.
fn normalise_body(body: &[u8], out: &mut Vec<u8>) -> usize {
    let start = out.len();
    let mut facets = 0;
    for line in body.split_inclusive(|&b| b == b'\n') {
        if first_word(line).eq_ignore_ascii_case(b"facet") {
            out.extend_from_slice(b"facet normal 0 0 0\n");
            facets += 1;
        } else {
            out.extend_from_slice(line);
        }
    }
    out[start..].make_ascii_lowercase();
    facets
}

/// The byte range after each `solid ...` line, up to its `endsolid` line, the next `solid` line or
/// the end. stl_io stops at the first `endsolid` too, so nothing it would read is cut off.
fn solid_bodies(text: &[u8]) -> Vec<Range<usize>> {
    let mut bodies: Vec<Range<usize>> = Vec::new();
    let mut open = false;
    let mut line_start = 0;
    while line_start < text.len() {
        let line_end = text[line_start..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(text.len(), |p| line_start + p);
        let word = first_word(&text[line_start..line_end]);
        let starts_solid = is_solid(word);
        let ends_solid = word.eq_ignore_ascii_case(b"endsolid");
        if open && (starts_solid || ends_solid) {
            if let Some(last) = bodies.last_mut() {
                last.end = line_start;
            }
            open = false;
        }
        if starts_solid {
            let body_start = (line_end + 1).min(text.len());
            bodies.push(body_start..text.len());
            open = true;
        }
        line_start = line_end + 1;
    }
    bodies
}

/// Binary STL: an 80-byte header that does not start with "solid", the triangle count, then 50
/// bytes per triangle with its unit normal. All parts go into one mesh.
pub fn write_binary<W: Write>(mut w: W, parts: &[NamedMesh]) -> io::Result<W> {
    let total: usize = parts.iter().map(|p| p.mesh.triangle_count()).sum();
    let count = u32::try_from(total)
        .map_err(|_| io::Error::other("binary STL cannot hold this many triangles"))?;
    let mut header = [0u8; 80];
    header[..12].copy_from_slice(b"tricky3D STL");
    w.write_all(&header)?;
    w.write_all(&count.to_le_bytes())?;
    let mut record = [0u8; 50];
    for p in parts {
        for t in p.mesh.indices.as_chunks::<3>().0 {
            let corners = [
                p.mesh.vertex(t[0]),
                p.mesh.vertex(t[1]),
                p.mesh.vertex(t[2]),
            ];
            let normal = triangle_normal(corners[0], corners[1], corners[2]);
            for (k, v) in normal.iter().chain(corners.iter().flatten()).enumerate() {
                record[k * 4..k * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            w.write_all(&record)?;
        }
    }
    Ok(w)
}

/// STL text: one solid holding every part, numbers in their shortest exact form.
pub fn write_ascii<W: Write>(mut w: W, title: &str, parts: &[NamedMesh]) -> io::Result<W> {
    let name: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = if name.trim().is_empty() {
        "tricky3d".to_string()
    } else {
        name.trim().to_string()
    };
    writeln!(w, "solid {name}")?;
    for p in parts {
        for t in p.mesh.indices.as_chunks::<3>().0 {
            let corners = [
                p.mesh.vertex(t[0]),
                p.mesh.vertex(t[1]),
                p.mesh.vertex(t[2]),
            ];
            let n = triangle_normal(corners[0], corners[1], corners[2]);
            writeln!(w, "  facet normal {} {} {}", n[0], n[1], n[2])?;
            writeln!(w, "    outer loop")?;
            for v in corners {
                writeln!(w, "      vertex {} {} {}", v[0], v[1], v[2])?;
            }
            writeln!(w, "    endloop")?;
            writeln!(w, "  endfacet")?;
        }
    }
    writeln!(w, "endsolid {name}")?;
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binary_header(start: &[u8], count: u32) -> Vec<u8> {
        let mut head = vec![b' '; 80];
        head[..start.len()].copy_from_slice(start);
        head.extend_from_slice(&count.to_le_bytes());
        head
    }

    #[test]
    fn detects_binary_even_with_a_solid_header() {
        let head = binary_header(b"solid part", 2);
        assert_eq!(detect(&head, 184), Kind::Binary(2));
        assert_eq!(
            detect(&head, 150),
            Kind::Truncated {
                expected: 2,
                found: 1
            }
        );
        assert_eq!(detect(&head, 134), Kind::OneShort(1));
        assert_eq!(detect(&head, 400), Kind::SizeMismatch { declared: 2 });
        assert_eq!(detect(b"  solid x\nfacet normal 0 0 1\n", 30), Kind::Text);
        assert_eq!(detect(b"hello", 5), Kind::NotStl);
        assert_eq!(detect(b"", 0), Kind::Empty);
    }

    #[test]
    fn other_files_are_not_mistaken_for_cut_off_binary_stl() {
        let html = b"<!DOCTYPE html>\n<html><head><title>404 Not Found</title></head><body><h1>Not Found</h1></body></html>\n";
        let step = b"ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('STEP AP214'),'2;1');\nFILE_NAME('part.step','2026-10-16',(''),(''),'','','');\n";
        for text in [&html[..], &step[..]] {
            assert!(text.len() >= 84);
            assert_eq!(detect(&text[..84], 5_000), Kind::NotStl);
        }
        assert_eq!(
            detect(b"SOLID PART\r\n  FACET NORMAL 0 0 1\r\n", 5_000),
            Kind::Text
        );
        assert_eq!(detect(b"\xEF\xBB\xBF Solid\n", 5_000), Kind::Text);
        // Binary data whose bytes at offset 80 make a count of over a billion triangles.
        let mut junk = binary_header(b"PK\x03\x04\x14\x00", 0);
        junk[80..84].copy_from_slice(&[0x10, 0x00, 0x9c, 0x41]);
        assert_eq!(detect(&junk, 23_000), Kind::NotStl);
        // A real 1,000,000-triangle STL cut off at 400 kB still says it ends early.
        assert_eq!(
            detect(&binary_header(b"part", 1_000_000), 400_000),
            Kind::Truncated {
                expected: 1_000_000,
                found: 7_998
            }
        );
    }

    fn facet(normal: &str, z: f32) -> String {
        format!(
            "  facet {normal}\n    outer loop\n      vertex 0 0 {z}\n      vertex 1 0 {z}\n      vertex 0 1 {z}\n    endloop\n  endfacet\n"
        )
    }

    #[test]
    fn non_finite_and_missing_normals_are_ignored() {
        let normals = [
            "normal nan nan nan",
            "normal -nan(ind) -nan(ind) -nan(ind)",
            "normal 1.#QNAN0 1.#QNAN0 1.#QNAN0",
            "normal inf -inf 1.#INF00",
            "normal",
            "normal 0 0",
            "normal 0 0 1",
        ];
        let text: String = normals
            .iter()
            .enumerate()
            .map(|(k, n)| facet(n, k as f32))
            .collect();
        let text = format!("solid odd normals\n{text}endsolid odd normals\n");
        let mesh = parse_text(text.as_bytes(), &mut Progress::silent()).unwrap();
        assert_eq!(mesh.triangle_count(), normals.len());
        assert_eq!(mesh.vertex(mesh.indices[3 * 6]), [0.0, 0.0, 6.0]);
    }

    #[test]
    fn errors_after_odd_normals_still_name_the_triangle() {
        let bad = facet("normal nan nan nan", 2.0).replace("vertex 1 0 2", "vertex 1 nan 2");
        let text = format!(
            "solid a\n{}{}{bad}endsolid a\n",
            facet("normal -nan(ind) 0 0", 0.0),
            facet("normal", 1.0)
        );
        let err = parse_text(text.as_bytes(), &mut Progress::silent()).unwrap_err();
        assert_eq!(err, "its STL text is not valid near triangle 3");
    }

    #[test]
    fn upper_case_text_loads_and_other_text_is_not_stl() {
        let upper = format!(
            "solid part\n{}endsolid part\n",
            facet("normal 0 0 1e0", 0.0)
        )
        .to_ascii_uppercase()
        .replace('\n', "\r\n");
        let mesh = parse_text(upper.as_bytes(), &mut Progress::silent()).unwrap();
        assert_eq!(mesh.triangle_count(), 1);

        let notes = "Solid modelling notes\nThe bracket needs 2 mm walls.\n";
        let err = parse_text(notes.as_bytes(), &mut Progress::silent()).unwrap_err();
        assert_eq!(err, "it is not an STL file");
    }

    #[test]
    fn text_with_several_solids_and_no_names_is_one_mesh() {
        let facet = |z: f32| {
            format!(
                "facet normal 0 0 1\nouter loop\nvertex 0 0 {z}\nvertex 1 0 {z}\nvertex 0 1 {z}\nendloop\nendfacet\n"
            )
        };
        let text = format!(
            "solid\n{}endsolid\nsolid second one\n{}{}endsolid second one\n",
            facet(0.0),
            facet(1.0),
            facet(2.0)
        );
        let mesh = parse_text(text.as_bytes(), &mut Progress::silent()).unwrap();
        assert_eq!(mesh.triangle_count(), 3);
        assert_eq!(mesh.vertex_count(), 9);
    }

    #[test]
    fn solid_names_in_a_windows_code_page_load() {
        // "Würfel" in Windows-1252: the 0xFC byte is not UTF-8.
        let text = b"solid W\xFCrfel\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid W\xFCrfel\nsolid two\nfacet normal 0 0 1\nouter loop\nvertex 0 0 1\nvertex 1 0 1\nvertex 0 1 1\nendloop\nendfacet\n  endsolid\t\xFC\n";
        let mesh = parse_text(text, &mut Progress::silent()).unwrap();
        assert_eq!(mesh.triangle_count(), 2);
    }

    #[test]
    fn text_errors_name_the_triangle() {
        let text = "solid a\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid a\n";
        let err = parse_text(text.as_bytes(), &mut Progress::silent()).unwrap_err();
        assert_eq!(err, "its STL text is not valid near triangle 1");
    }

    #[test]
    fn binary_header_never_starts_with_solid() {
        let mesh = crate::mesh::make_box([0.0; 3], [1.0; 3]);
        let out = write_binary(
            Vec::new(),
            &[NamedMesh {
                name: "a",
                mesh: &mesh,
            }],
        )
        .unwrap();
        assert!(!out.starts_with(b"solid"));
        assert_eq!(out.len(), 84 + 50 * 12);
        assert_eq!(detect(&out[..84], out.len() as u64), Kind::Binary(12));
    }
}
