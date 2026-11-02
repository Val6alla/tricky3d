//! 3D Studio (.3ds), the binary chunk format: each named object's triangle mesh (points in world
//! coordinates, as 3DS stores them; the object's own frame is not applied again), with the diffuse
//! colour of the materials its faces use. One material gives the part its colour; several give a
//! colour per triangle. 3DS names no unit, so the numbers are millimetres.
//!
//! Export writes one object per part. A 3DS object holds at most 65,535 points and faces, so a
//! larger part is written as several objects; names are cut to the format's ten characters.

use std::collections::HashMap;
use std::io::{self, Seek, SeekFrom, Write};

use super::{Loaded, NamedMesh, Object};
use crate::mesh::{fmt_count, weld_indexed};
use crate::progress::{Progress, Stage};

const MAIN: u16 = 0x4D4D;
const VERSION: u16 = 0x0002;
const EDITOR: u16 = 0x3D3D;
const MESH_VERSION: u16 = 0x3D3E;
const MASTER_SCALE: u16 = 0x0100;
const MATERIAL: u16 = 0xAFFF;
const MATERIAL_NAME: u16 = 0xA000;
const DIFFUSE: u16 = 0xA020;
const COLOR_F: u16 = 0x0010;
const COLOR_24: u16 = 0x0011;
const LIN_COLOR_24: u16 = 0x0012;
const LIN_COLOR_F: u16 = 0x0013;
const OBJECT: u16 = 0x4000;
const TRIMESH: u16 = 0x4100;
const POINTS: u16 = 0x4110;
const FACES: u16 = 0x4120;
const FACE_MATERIAL: u16 = 0x4130;

/// A chunk: its id, and its content after the 6-byte header.
struct Chunk<'a> {
    id: u16,
    data: &'a [u8],
}

/// The chunks one after another in `data`; a chunk whose length runs past the end is cut there.
fn chunks(data: &[u8]) -> impl Iterator<Item = Chunk<'_>> {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        if at + 6 > data.len() {
            return None;
        }
        let id = u16::from_le_bytes([data[at], data[at + 1]]);
        let len = u32::from_le_bytes(data[at + 2..at + 6].try_into().unwrap()) as usize;
        if len < 6 {
            return None;
        }
        let end = (at + len).min(data.len());
        let chunk = Chunk {
            id,
            data: &data[at + 6..end],
        };
        at = end;
        Some(chunk)
    })
}

fn cstring(data: &[u8]) -> (String, &[u8]) {
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    let name = String::from_utf8_lossy(&data[..end]).into_owned();
    (name, &data[(end + 1).min(data.len())..])
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn f32_at(data: &[u8], at: usize) -> Option<f32> {
    Some(f32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// The colour in a colour chunk's sub-chunks (24-bit ones before float ones).
fn color(data: &[u8]) -> Option<[u8; 3]> {
    let mut float = None;
    for c in chunks(data) {
        match c.id {
            COLOR_24 | LIN_COLOR_24 if c.data.len() >= 3 => {
                return Some([c.data[0], c.data[1], c.data[2]]);
            }
            COLOR_F | LIN_COLOR_F if float.is_none() => {
                let f = |k: usize| {
                    f32_at(c.data, 4 * k).map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8)
                };
                if let (Some(r), Some(g), Some(b)) = (f(0), f(1), f(2)) {
                    float = Some([r, g, b]);
                }
            }
            _ => {}
        }
    }
    float
}

pub fn read(bytes: &[u8], progress: &mut Progress) -> Result<Loaded, String> {
    let Some(main) = chunks(bytes).next().filter(|c| c.id == MAIN) else {
        return Err("it is not a 3DS file".into());
    };
    let mut materials: HashMap<String, [u8; 3]> = HashMap::new();
    let mut raw: Vec<(String, RawMesh)> = Vec::new();
    let mut warnings = Vec::new();
    for editor in chunks(main.data).filter(|c| c.id == EDITOR) {
        for c in chunks(editor.data) {
            match c.id {
                MATERIAL => {
                    let mut name = None;
                    let mut diffuse = None;
                    for m in chunks(c.data) {
                        match m.id {
                            MATERIAL_NAME => name = Some(cstring(m.data).0),
                            DIFFUSE => diffuse = color(m.data),
                            _ => {}
                        }
                    }
                    if let (Some(n), Some(d)) = (name, diffuse) {
                        materials.insert(n, d);
                    }
                }
                OBJECT => {
                    let (name, rest) = cstring(c.data);
                    for t in chunks(rest).filter(|t| t.id == TRIMESH) {
                        if let Some(mesh) = trimesh(t.data, &mut warnings)? {
                            raw.push((name.clone(), mesh));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    progress.report(Stage::Parse, 1.0);
    let count = raw.len();
    let mut objects = Vec::with_capacity(count);
    for (k, (name, (positions, indices, face_materials))) in raw.into_iter().enumerate() {
        let used: Vec<Option<[u8; 3]>> = face_materials
            .iter()
            .map(|m| m.as_ref().and_then(|m| materials.get(m).copied()))
            .collect();
        let first = used.iter().flatten().next().copied();
        let several = used.iter().any(|&c| c != used[0]);
        let colors = several.then(|| {
            used.iter()
                .flat_map(|c| c.unwrap_or([204, 204, 204]))
                .collect()
        });
        objects.push(Object {
            name: super::clean_name(Some(&name)),
            mesh: weld_indexed(&positions, &indices)?,
            color: first,
            colors,
            ..Object::default()
        });
        progress.report(Stage::Weld, (k + 1) as f64 / count.max(1) as f64);
    }
    if !warnings.is_empty() {
        warnings.dedup();
    }
    Ok(Loaded {
        object_count: objects.len(),
        splittable: objects.len() > 1,
        objects,
        warnings,
        unit: None,
    })
}

type RawMesh = (Vec<f32>, Vec<u32>, Vec<Option<String>>);

/// A triangle mesh chunk's points, faces and each face's material name.
fn trimesh(data: &[u8], warnings: &mut Vec<String>) -> Result<Option<RawMesh>, String> {
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    let mut materials: Vec<Option<String>> = Vec::new();
    for c in chunks(data) {
        match c.id {
            POINTS => {
                let n = u16_at(c.data, 0).ok_or("its 3DS data ends early")? as usize;
                if c.data.len() < 2 + 12 * n {
                    return Err("its 3DS data ends early (a point list is cut off)".into());
                }
                for k in 0..3 * n {
                    positions.push(f32_at(c.data, 2 + 4 * k).unwrap_or(0.0));
                }
            }
            FACES => {
                let n = u16_at(c.data, 0).ok_or("its 3DS data ends early")? as usize;
                if c.data.len() < 2 + 8 * n {
                    return Err("its 3DS data ends early (a face list is cut off)".into());
                }
                for f in 0..n {
                    for k in 0..3 {
                        indices.push(u16_at(c.data, 2 + 8 * f + 2 * k).unwrap_or(0) as u32);
                    }
                }
                materials = vec![None; n];
                for m in chunks(&c.data[2 + 8 * n..]).filter(|m| m.id == FACE_MATERIAL) {
                    let (name, rest) = cstring(m.data);
                    let count = u16_at(rest, 0).unwrap_or(0) as usize;
                    for i in 0..count {
                        if let Some(f) = u16_at(rest, 2 + 2 * i)
                            && let Some(slot) = materials.get_mut(f as usize)
                        {
                            *slot = Some(name.clone());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if indices.is_empty() {
        return Ok(None);
    }
    let count = positions.len() / 3;
    if indices.iter().any(|&i| i as usize >= count) {
        warnings.push(format!(
            "Faces that refer to points that do not exist were left out (an object has {} points).",
            fmt_count(count as u64)
        ));
        let mut kept_indices = Vec::new();
        let mut kept_materials = Vec::new();
        for (t, m) in indices.as_chunks::<3>().0.iter().zip(&materials) {
            if t.iter().all(|&i| (i as usize) < count) {
                kept_indices.extend_from_slice(t);
                kept_materials.push(m.clone());
            }
        }
        indices = kept_indices;
        materials = kept_materials;
    }
    Ok(Some((positions, indices, materials)))
}

// ---------------------------------------------------------------------------------------------
// Export

/// Most points or faces one 3DS object holds.
const MAX_ITEMS: usize = 65_535;

/// Starts a chunk and returns where its length goes.
fn begin<W: Write + Seek>(w: &mut W, id: u16) -> io::Result<u64> {
    let at = w.stream_position()?;
    w.write_all(&id.to_le_bytes())?;
    w.write_all(&0u32.to_le_bytes())?;
    Ok(at)
}

/// Writes the length of the chunk begun at `at`, now that its content is written.
fn end<W: Write + Seek>(w: &mut W, at: u64) -> io::Result<()> {
    let here = w.stream_position()?;
    let len = u32::try_from(here - at)
        .map_err(|_| io::Error::other("the 3DS file would be larger than 4 GB"))?;
    w.seek(SeekFrom::Start(at + 2))?;
    w.write_all(&len.to_le_bytes())?;
    w.seek(SeekFrom::Start(here))?;
    Ok(())
}

/// The part's triangles in pieces of at most MAX_ITEMS points and faces, each with its own points.
/// A piece of a part as one 3DS object: its points and its faces over them.
type Piece = (Vec<[f32; 3]>, Vec<[u16; 3]>);

fn pieces(mesh: &crate::mesh::Mesh) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut map: HashMap<u32, u16> = HashMap::new();
    let mut points: Vec<[f32; 3]> = Vec::new();
    let mut faces: Vec<[u16; 3]> = Vec::new();
    for t in mesh.indices.as_chunks::<3>().0 {
        let new = t.iter().filter(|v| !map.contains_key(v)).count();
        if points.len() + new > MAX_ITEMS || faces.len() == MAX_ITEMS {
            out.push((std::mem::take(&mut points), std::mem::take(&mut faces)));
            map.clear();
        }
        let face = t.map(|v| {
            *map.entry(v).or_insert_with(|| {
                points.push(mesh.vertex(v));
                (points.len() - 1) as u16
            })
        });
        faces.push(face);
    }
    if !faces.is_empty() {
        out.push((points, faces));
    }
    out
}

/// A 3DS object name: at most ten characters, unique in the file.
fn object_name(name: &str, taken: &mut std::collections::HashSet<String>) -> String {
    let clean: String = name
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .collect();
    let clean = if clean.trim().is_empty() {
        "Part".to_string()
    } else {
        clean.trim().to_string()
    };
    let cut = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    let mut out = cut(&clean, 10);
    let mut n = 2;
    while !taken.insert(out.to_ascii_lowercase()) {
        let suffix = format!("~{n}");
        out = format!("{}{suffix}", cut(&clean, 10 - suffix.len()));
        n += 1;
    }
    out
}

/// 3DS with one object per part (more for parts over 65,535 points or faces).
pub fn write<W: Write + Seek>(mut w: W, parts: &[NamedMesh]) -> io::Result<W> {
    let main = begin(&mut w, MAIN)?;
    let version = begin(&mut w, VERSION)?;
    w.write_all(&3u32.to_le_bytes())?;
    end(&mut w, version)?;
    let editor = begin(&mut w, EDITOR)?;
    let mesh_version = begin(&mut w, MESH_VERSION)?;
    w.write_all(&3u32.to_le_bytes())?;
    end(&mut w, mesh_version)?;
    let scale = begin(&mut w, MASTER_SCALE)?;
    w.write_all(&1f32.to_le_bytes())?;
    end(&mut w, scale)?;
    let mut taken = std::collections::HashSet::new();
    for p in parts {
        for (points, faces) in pieces(p.mesh) {
            let object = begin(&mut w, OBJECT)?;
            w.write_all(object_name(p.name, &mut taken).as_bytes())?;
            w.write_all(&[0])?;
            let mesh = begin(&mut w, TRIMESH)?;
            let list = begin(&mut w, POINTS)?;
            w.write_all(&(points.len() as u16).to_le_bytes())?;
            for q in &points {
                for x in q {
                    w.write_all(&x.to_le_bytes())?;
                }
            }
            end(&mut w, list)?;
            let list = begin(&mut w, FACES)?;
            w.write_all(&(faces.len() as u16).to_le_bytes())?;
            for f in &faces {
                for v in f {
                    w.write_all(&v.to_le_bytes())?;
                }
                // Edge visibility flags: all three edges shown.
                w.write_all(&7u16.to_le_bytes())?;
            }
            end(&mut w, list)?;
            end(&mut w, mesh)?;
            end(&mut w, object)?;
        }
    }
    end(&mut w, editor)?;
    end(&mut w, main)?;
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{Mesh, bounds, stats};
    use std::io::Cursor;

    fn cube(size: f32, at: [f32; 3]) -> Mesh {
        let mut positions = Vec::new();
        for k in 0..8 {
            positions.extend([
                at[0] + if k & 1 == 0 { 0.0 } else { size },
                at[1] + if k & 2 == 0 { 0.0 } else { size },
                at[2] + if k & 4 == 0 { 0.0 } else { size },
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
        let indices = quads
            .iter()
            .flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]])
            .collect();
        Mesh { positions, indices }
    }

    #[test]
    fn writes_and_reads_two_parts() {
        let (a, b) = (cube(10.0, [0.0; 3]), cube(5.0, [20.0, 0.0, 0.0]));
        let parts = [
            NamedMesh {
                name: "A long part name",
                mesh: &a,
            },
            NamedMesh {
                name: "A long part name",
                mesh: &b,
            },
        ];
        let bytes = write(Cursor::new(Vec::new()), &parts).unwrap().into_inner();
        let loaded = read(&bytes, &mut Progress::silent()).unwrap();
        assert_eq!(loaded.objects.len(), 2);
        assert_eq!(loaded.objects[0].name.as_deref(), Some("A long par"));
        assert_eq!(loaded.objects[1].name.as_deref(), Some("A long p~2"));
        assert_eq!(
            loaded.objects[0].mesh,
            weld_indexed(&a.positions, &a.indices).unwrap()
        );
        assert!((stats(&loaded.objects[1].mesh).volume - 125.0).abs() < 1e-4);
        assert_eq!(
            bounds(&loaded.objects[1].mesh).unwrap(),
            [[20.0, 0.0, 0.0], [25.0, 5.0, 5.0]]
        );
    }

    #[test]
    fn a_part_over_the_limit_is_split_into_objects() {
        // 70,000 separate triangles: 210,000 points.
        let mut mesh = Mesh::default();
        for t in 0..70_000u32 {
            let x = t as f32;
            mesh.positions
                .extend([x, 0.0, 0.0, x, 1.0, 0.0, x, 0.0, 1.0]);
            mesh.indices.extend([3 * t, 3 * t + 1, 3 * t + 2]);
        }
        let parts = [NamedMesh {
            name: "big",
            mesh: &mesh,
        }];
        let bytes = write(Cursor::new(Vec::new()), &parts).unwrap().into_inner();
        let loaded = read(&bytes, &mut Progress::silent()).unwrap();
        assert_eq!(loaded.objects.len(), 4);
        let total: usize = loaded.objects.iter().map(|o| o.mesh.triangle_count()).sum();
        assert_eq!(total, 70_000);
    }

    #[test]
    fn face_materials_give_colours() {
        // One object, two faces, each with its own material.
        let mut file = Cursor::new(Vec::new());
        let w = &mut file;
        let main = begin(w, MAIN).unwrap();
        let editor = begin(w, EDITOR).unwrap();
        for (name, rgb) in [("red", [255u8, 0, 0]), ("blue", [0, 0, 255])] {
            let m = begin(w, MATERIAL).unwrap();
            let n = begin(w, MATERIAL_NAME).unwrap();
            w.write_all(name.as_bytes()).unwrap();
            w.write_all(&[0]).unwrap();
            end(w, n).unwrap();
            let d = begin(w, DIFFUSE).unwrap();
            let c = begin(w, COLOR_24).unwrap();
            w.write_all(&rgb).unwrap();
            end(w, c).unwrap();
            end(w, d).unwrap();
            end(w, m).unwrap();
        }
        let o = begin(w, OBJECT).unwrap();
        w.write_all(b"quad\0").unwrap();
        let t = begin(w, TRIMESH).unwrap();
        let p = begin(w, POINTS).unwrap();
        w.write_all(&4u16.to_le_bytes()).unwrap();
        for q in [
            [0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ] {
            for x in q {
                w.write_all(&x.to_le_bytes()).unwrap();
            }
        }
        end(w, p).unwrap();
        let f = begin(w, FACES).unwrap();
        w.write_all(&2u16.to_le_bytes()).unwrap();
        for face in [[0u16, 1, 2, 7], [0, 2, 3, 7]] {
            for v in face {
                w.write_all(&v.to_le_bytes()).unwrap();
            }
        }
        for (name, face) in [("red", 0u16), ("blue", 1)] {
            let m = begin(w, FACE_MATERIAL).unwrap();
            w.write_all(name.as_bytes()).unwrap();
            w.write_all(&[0]).unwrap();
            w.write_all(&1u16.to_le_bytes()).unwrap();
            w.write_all(&face.to_le_bytes()).unwrap();
            end(w, m).unwrap();
        }
        end(w, f).unwrap();
        end(w, t).unwrap();
        end(w, o).unwrap();
        end(w, editor).unwrap();
        end(w, main).unwrap();
        let loaded = read(&file.into_inner(), &mut Progress::silent()).unwrap();
        let o = &loaded.objects[0];
        assert_eq!(o.color, Some([255, 0, 0]));
        assert_eq!(o.colors.as_deref(), Some(&[255, 0, 0, 0, 0, 255][..]));
        assert!(read(b"not a 3ds file", &mut Progress::silent()).is_err());
    }
}
