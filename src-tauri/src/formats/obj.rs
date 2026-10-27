//! Wavefront OBJ through tobj: polygons are triangulated, points and lines ignored, and material
//! libraries are never opened (only geometry matters, and the file may sit next to anything).

use std::borrow::Cow;
use std::io::{self, BufRead, Read, Write};

use super::{Loaded, NamedMesh, Object, clean_name};
use crate::mesh::weld_indexed;
use crate::progress::{Progress, Stage};

/// One object per tobj model, or all of them in one welded mesh with `merge`.
pub fn read(bytes: &[u8], merge: bool, progress: &mut Progress) -> Result<Loaded, String> {
    // tobj reads line by line and gives up on bytes that are not UTF-8, which old exporters
    // write in comments and names; replace them instead of failing.
    let text: Cow<str> = String::from_utf8_lossy(bytes);
    let models = match load(&text, progress) {
        // Normals, texture coordinates and materials are not used, but tobj rejects the
        // "-nan(ind)" or "1.#QNAN0" some writers put there for degenerate faces, and a `usemtl`
        // without a name: neutralise those lines and read again.
        Err(
            tobj::LoadError::NormalParseError
            | tobj::LoadError::TexcoordParseError
            | tobj::LoadError::MaterialParseError,
        ) => load(&tidy(&text), progress),
        loaded => loaded,
    }
    .map_err(|e| format!("its OBJ data is not valid ({e})"))?;
    let models: Vec<tobj::Model> = models
        .into_iter()
        .filter(|m| m.mesh.indices.len() >= 3)
        .collect();
    let object_count = models.len();

    let objects = if merge {
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for m in &models {
            let base = (positions.len() / 3) as u32;
            positions.extend_from_slice(&m.mesh.positions);
            indices.extend(m.mesh.indices.iter().map(|i| base + i));
        }
        let mesh = weld_indexed(&positions, &indices)?;
        progress.report(Stage::Weld, 1.0);
        vec![Object {
            name: None,
            matrix: None,
            mesh,
        }]
    } else {
        let mut objects = Vec::with_capacity(models.len());
        for (k, m) in models.iter().enumerate() {
            objects.push(Object {
                name: clean_name(Some(&m.name)).filter(|n| n != "unnamed_object"),
                matrix: None,
                mesh: weld_indexed(&m.mesh.positions, &m.mesh.indices)?,
            });
            progress.report(Stage::Weld, (k + 1) as f64 / models.len() as f64);
        }
        objects
    };
    Ok(Loaded {
        objects,
        object_count,
        splittable: object_count > 1 && !merge,
        warnings: Vec::new(),
    })
}

fn load(text: &str, progress: &mut Progress) -> Result<Vec<tobj::Model>, tobj::LoadError> {
    let options = tobj::LoadOptions {
        triangulate: true,
        single_index: true,
        ignore_points: true,
        ignore_lines: true,
    };
    let mut reader = ProgressReader {
        data: text.as_bytes(),
        pos: 0,
        reported: 0,
        progress,
    };
    let (models, _materials) = tobj::load_obj_buf(&mut reader, &options, |_| {
        Err(tobj::LoadError::OpenFileFailed)
    })?;
    Ok(models)
}

/// The text with every `vn` line as "vn 0 0 0", every `vt` line as "vt 0 0" and every `usemtl`
/// without a name blank, line for line, so the faces still refer to the same entries.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let mut words = line.split_whitespace();
        match (words.next(), words.next()) {
            (Some("vn"), _) => out.push_str("vn 0 0 0\n"),
            (Some("vt"), _) => out.push_str("vt 0 0\n"),
            (Some("usemtl"), None) => out.push('\n'),
            _ => out.push_str(line),
        }
    }
    out
}

/// Feeds tobj from memory and reports how far parsing has got.
struct ProgressReader<'d, 'p, 'q> {
    data: &'d [u8],
    pos: usize,
    reported: usize,
    progress: &'p mut Progress<'q>,
}

impl Read for ProgressReader<'_, '_, '_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = (&self.data[self.pos..]).read(buf)?;
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for ProgressReader<'_, '_, '_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        let end = (self.pos + (64 << 10)).min(self.data.len());
        Ok(&self.data[self.pos..end])
    }

    fn consume(&mut self, n: usize) {
        self.pos = (self.pos + n).min(self.data.len());
        if self.pos - self.reported >= 1 << 20 {
            self.reported = self.pos;
            self.progress.report(
                Stage::Parse,
                self.pos as f64 / self.data.len().max(1) as f64,
            );
        }
    }
}

/// OBJ with one `o` per part and 1-based indices counted across the whole file.
pub fn write<W: Write>(mut w: W, parts: &[NamedMesh]) -> io::Result<W> {
    writeln!(w, "# tricky3D")?;
    let mut base = 1u64;
    for (k, p) in parts.iter().enumerate() {
        let name = clean_name(Some(p.name)).unwrap_or_else(|| format!("Object {}", k + 1));
        writeln!(w, "o {name}")?;
        for v in p.mesh.positions.as_chunks::<3>().0 {
            writeln!(w, "v {} {} {}", v[0], v[1], v[2])?;
        }
        for t in p.mesh.indices.as_chunks::<3>().0 {
            writeln!(
                w,
                "f {} {} {}",
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
    fn unreadable_normals_and_texture_coordinates_are_ignored() {
        for bad in [
            "-nan(ind) -nan(ind) -nan(ind)",
            "1.#QNAN0 1.#QNAN0 1.#QNAN0",
        ] {
            let text = format!(
                "v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 1.#IND00 0\nvn {bad}\nf 1/1/1 2/1/1 3/1/1\n"
            );
            let loaded = read(text.as_bytes(), false, &mut Progress::silent())
                .unwrap_or_else(|e| panic!("{bad}: {e}"));
            assert_eq!(loaded.objects[0].mesh.triangle_count(), 1, "{bad}");
        }
        // Positions are still checked.
        let text = b"v 0 0 0\nv 1 -nan(ind) 0\nv 0 1 0\nvn 1.#QNAN0 0 0\nf 1//1 2//1 3//1\n";
        let err = read(text, false, &mut Progress::silent()).unwrap_err();
        assert!(err.starts_with("its OBJ data is not valid"), "{err}");
    }

    #[test]
    fn a_usemtl_without_a_name_is_ignored() {
        let text = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nusemtl \nf 1 2 3\nusemtl\nf 1 2 4\n";
        let loaded = read(text, false, &mut Progress::silent()).unwrap();
        assert_eq!(loaded.objects[0].mesh.triangle_count(), 2);
    }
}
