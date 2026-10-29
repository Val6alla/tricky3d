//! OFF (Object File Format), text only. The header keyword may carry the usual prefixes (C, N, ST
//! for colours, normals and texture coordinates, as in "COFF" or "NOFF"); the counts follow on the
//! same line or the next. Vertices take their first three numbers, faces "n i1 … in" and any colour
//! after the corners is ignored. Polygons are split into a fan of triangles. `#` starts a comment.
//! Export writes text OFF with every part in one mesh.

use std::io::{self, Write};

use super::{Loaded, NamedMesh};
use crate::mesh::{fmt_count, weld_indexed};
use crate::progress::{Progress, Stage};

pub fn read(bytes: &[u8], progress: &mut Progress) -> Result<Loaded, String> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text
        .lines()
        .map(|l| l.split('#').next().unwrap_or_default())
        .filter(|l| !l.trim().is_empty());

    let first = lines.next().ok_or("it is not an OFF file")?;
    let mut words = first.split_whitespace();
    let keyword = words.next().unwrap_or_default();
    let base = keyword.trim_start_matches(['S', 'T', 'C', 'N']);
    if base != "OFF" {
        return Err(if base == "4OFF" || base == "nOFF" {
            "its OFF data has more than three dimensions".into()
        } else {
            "it is not an OFF file".into()
        });
    }
    let rest: Vec<&str> = words.collect();
    if rest.first() == Some(&"BINARY") {
        return Err("binary OFF files cannot be opened (only text OFF)".into());
    }
    let counts_line = if rest.is_empty() {
        lines.next().ok_or("the file ends early")?
    } else {
        first.split_once(keyword).map_or("", |(_, r)| r)
    };
    let counts: Vec<usize> = counts_line
        .split_whitespace()
        .take(2)
        .map(|w| w.parse::<usize>())
        .collect::<Result<_, _>>()
        .map_err(|_| "its OFF header is not valid".to_string())?;
    let [vertex_count, face_count] = counts[..] else {
        return Err("its OFF header is not valid".into());
    };
    let total = (vertex_count + face_count).max(1);

    let mut positions: Vec<f32> = Vec::with_capacity(vertex_count.min(bytes.len() / 6) * 3);
    for i in 0..vertex_count {
        let bad = || {
            format!(
                "its OFF data is not valid at vertex {}",
                fmt_count(i as u64 + 1)
            )
        };
        let line = lines.next().ok_or_else(|| {
            format!(
                "the file ends early (expected {} vertices, found {})",
                fmt_count(vertex_count as u64),
                fmt_count(i as u64)
            )
        })?;
        let p: Vec<f32> = line
            .split_whitespace()
            .take(3)
            .map(|w| w.parse::<f32>())
            .collect::<Result<_, _>>()
            .map_err(|_| bad())?;
        if p.len() < 3 {
            return Err(bad());
        }
        positions.extend_from_slice(&p);
        if (i + 1).is_multiple_of(65_536) {
            progress.report(Stage::Parse, (i + 1) as f64 / total as f64);
        }
    }

    let mut indices: Vec<u32> = Vec::with_capacity(face_count.min(bytes.len() / 6) * 3);
    let mut corners: Vec<u32> = Vec::new();
    let mut skipped = 0usize;
    for f in 0..face_count {
        let bad = || {
            format!(
                "its OFF data is not valid at face {}",
                fmt_count(f as u64 + 1)
            )
        };
        let line = lines.next().ok_or_else(|| {
            format!(
                "the file ends early (expected {} faces, found {})",
                fmt_count(face_count as u64),
                fmt_count(f as u64)
            )
        })?;
        let mut words = line.split_whitespace();
        let n: usize = words.next().and_then(|w| w.parse().ok()).ok_or_else(bad)?;
        corners.clear();
        for _ in 0..n {
            let i: u32 = words.next().and_then(|w| w.parse().ok()).ok_or_else(bad)?;
            if i as usize >= vertex_count {
                return Err(format!(
                    "face {} refers to a vertex that does not exist",
                    fmt_count(f as u64 + 1)
                ));
            }
            corners.push(i);
        }
        if n < 3 {
            skipped += 1;
            continue;
        }
        for k in 1..n - 1 {
            indices.extend_from_slice(&[corners[0], corners[k], corners[k + 1]]);
        }
        if (vertex_count + f + 1).is_multiple_of(65_536) {
            progress.report(Stage::Parse, (vertex_count + f + 1) as f64 / total as f64);
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

/// Text OFF with all parts merged into one mesh (OFF holds one object and no units; the numbers
/// are millimetres, in their shortest exact form).
pub fn write<W: Write>(mut w: W, parts: &[NamedMesh]) -> io::Result<W> {
    let vertices: usize = parts.iter().map(|p| p.mesh.vertex_count()).sum();
    let faces: usize = parts.iter().map(|p| p.mesh.triangle_count()).sum();
    writeln!(w, "OFF\n# tricky3D, millimetres\n{vertices} {faces} 0")?;
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

    fn read_text(text: &str) -> Result<Loaded, String> {
        read(text.as_bytes(), &mut Progress::silent())
    }

    #[test]
    fn reads_a_tetrahedron_with_comments_colours_and_a_quad() {
        let text = "OFF\n# a comment\n5 3 0\n0 0 0\n1 0 0\n0 1 0 255 0 0\n0 0 1\n1 1 0\n\
                    3 0 2 1 0.5 0.5 0.5\n3 0 1 3\n4 1 4 2 3\n";
        let loaded = read_text(text).unwrap();
        let mesh = &loaded.objects[0].mesh;
        assert_eq!(mesh.triangle_count(), 4);
        assert_eq!(mesh.positions.len(), 15);
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn counts_may_share_the_keyword_line_and_prefixes_are_accepted() {
        let loaded = read_text("COFF 3 1 0\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n").unwrap();
        assert_eq!(loaded.objects[0].mesh.triangle_count(), 1);
    }

    #[test]
    fn errors_name_what_is_wrong() {
        assert_eq!(read_text("solid x\n").unwrap_err(), "it is not an OFF file");
        assert_eq!(
            read_text("OFF\n3 1 0\n0 0 0\n1 0 0\n").unwrap_err(),
            "the file ends early (expected 3 vertices, found 2)"
        );
        assert_eq!(
            read_text("OFF\n3 1 0\n0 0 0\n1 0 0\n0 1 0\n3 0 1 7\n").unwrap_err(),
            "face 1 refers to a vertex that does not exist"
        );
        assert_eq!(
            read_text("OFF BINARY\n").unwrap_err(),
            "binary OFF files cannot be opened (only text OFF)"
        );
    }

    #[test]
    fn faces_with_fewer_than_three_corners_are_skipped_with_a_warning() {
        let loaded = read_text("OFF\n3 2 0\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n2 0 1\n").unwrap();
        assert_eq!(loaded.objects[0].mesh.triangle_count(), 1);
        assert_eq!(loaded.warnings.len(), 1);
    }
}
