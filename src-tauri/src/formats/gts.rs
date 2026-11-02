//! GTS, the GNU Triangulated Surface text format: a header line with the point, edge and triangle
//! counts, then the points (x y z), the edges (two 1-based point numbers) and the triangles (three
//! 1-based edge numbers). A triangle's corners are, in order, the end of its first edge that the
//! second edge does not share, the point the two share, and the second edge's other end (as the GTS
//! library reads them), which sets the way it faces. Anything after the numbers a line needs is
//! ignored, and so are lines starting with `#`. GTS names no unit, so the numbers are millimetres.
//!
//! Export writes every part into one surface, each edge once.

use std::collections::HashMap;
use std::io::{self, Write};

use super::{Loaded, NamedMesh};
use crate::mesh::{fmt_count, weld_indexed};
use crate::progress::{Progress, Stage};

pub fn read(bytes: &[u8], progress: &mut Progress) -> Result<Loaded, String> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
    let not_gts = || "it is not a GTS file (its first line is not three counts)".to_string();
    let header = lines.next().ok_or_else(not_gts)?;
    let counts: Vec<usize> = header
        .split_whitespace()
        .take(3)
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|_| not_gts())?;
    let [np, ne, nf] = counts[..] else {
        return Err(not_gts());
    };
    let total = (np + ne + nf).max(1);
    let ends_early = |what: &str, expected: usize, found: usize| {
        format!(
            "the file ends early (expected {} {what}, found {})",
            fmt_count(expected as u64),
            fmt_count(found as u64)
        )
    };
    let mut positions: Vec<f32> = Vec::with_capacity(3 * np.min(bytes.len() / 6));
    for i in 0..np {
        let bad = || {
            format!(
                "its GTS data is not valid at point {}",
                fmt_count(i as u64 + 1)
            )
        };
        let line = lines.next().ok_or_else(|| ends_early("points", np, i))?;
        let p: Vec<f32> = line
            .split_whitespace()
            .take(3)
            .map(str::parse)
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
    let mut edges: Vec<[u32; 2]> = Vec::with_capacity(ne.min(bytes.len() / 4));
    for i in 0..ne {
        let bad = || {
            format!(
                "its GTS data is not valid at edge {}",
                fmt_count(i as u64 + 1)
            )
        };
        let line = lines.next().ok_or_else(|| ends_early("edges", ne, i))?;
        let v: Vec<usize> = line
            .split_whitespace()
            .take(2)
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|_| bad())?;
        match v[..] {
            [a, b] if (1..=np).contains(&a) && (1..=np).contains(&b) => {
                edges.push([a as u32 - 1, b as u32 - 1]);
            }
            _ => return Err(bad()),
        }
    }
    let mut indices: Vec<u32> = Vec::with_capacity(3 * nf.min(bytes.len() / 6));
    let mut skipped = 0usize;
    for i in 0..nf {
        let bad = || {
            format!(
                "its GTS data is not valid at triangle {}",
                fmt_count(i as u64 + 1)
            )
        };
        let line = lines.next().ok_or_else(|| ends_early("triangles", nf, i))?;
        let e: Vec<usize> = line
            .split_whitespace()
            .take(3)
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|_| bad())?;
        let [e1, e2, e3] = e[..] else {
            return Err(bad());
        };
        if ![e1, e2, e3].iter().all(|e| (1..=ne).contains(e)) {
            return Err(bad());
        }
        match corners(edges[e1 - 1], edges[e2 - 1]) {
            Some(t) => indices.extend_from_slice(&t),
            None => skipped += 1,
        }
        if (np + ne + i + 1).is_multiple_of(65_536) {
            progress.report(Stage::Parse, (np + ne + i + 1) as f64 / total as f64);
        }
    }
    let mesh = weld_indexed(&positions, &indices)?;
    progress.report(Stage::Weld, 1.0);
    let mut loaded = Loaded::single(mesh);
    if skipped > 0 {
        loaded.warnings.push(format!(
            "{} triangles whose first two edges do not meet were left out.",
            fmt_count(skipped as u64)
        ));
    }
    Ok(loaded)
}

/// A triangle's corners from its first two edges: the first edge's far end, the shared point, the
/// second edge's far end. None when the edges share no point (or are the same edge).
fn corners(e1: [u32; 2], e2: [u32; 2]) -> Option<[u32; 3]> {
    let shared = if e1.contains(&e2[0]) {
        e2[0]
    } else if e1.contains(&e2[1]) {
        e2[1]
    } else {
        return None;
    };
    let p = if e1[0] == shared { e1[1] } else { e1[0] };
    let q = if e2[0] == shared { e2[1] } else { e2[0] };
    (p != shared && q != shared && p != q).then_some([p, shared, q])
}

/// Every part in one GTS surface: triangle (a, b, c) as its edges a–b, b–c, c–a.
pub fn write<W: Write>(mut w: W, parts: &[NamedMesh]) -> io::Result<W> {
    let points: usize = parts.iter().map(|p| p.mesh.vertex_count()).sum();
    let mut edge_of: HashMap<(u64, u64), u64> = HashMap::new();
    let mut edges: Vec<(u64, u64)> = Vec::new();
    let mut faces: Vec<[u64; 3]> = Vec::new();
    let mut base = 0u64;
    for p in parts {
        for t in p.mesh.indices.as_chunks::<3>().0 {
            let v = t.map(|i| base + i as u64);
            let face = [0, 1, 2].map(|k| {
                let (a, b) = (v[k], v[(k + 1) % 3]);
                let key = (a.min(b), a.max(b));
                *edge_of.entry(key).or_insert_with(|| {
                    edges.push(key);
                    edges.len() as u64
                })
            });
            faces.push(face);
        }
        base += p.mesh.vertex_count() as u64;
    }
    writeln!(
        w,
        "{points} {} {} GtsSurface GtsFace GtsEdge GtsVertex",
        edges.len(),
        faces.len()
    )?;
    for p in parts {
        for q in p.mesh.positions.as_chunks::<3>().0 {
            writeln!(w, "{} {} {}", q[0], q[1], q[2])?;
        }
    }
    for (a, b) in &edges {
        writeln!(w, "{} {}", a + 1, b + 1)?;
    }
    for f in &faces {
        writeln!(w, "{} {} {}", f[0], f[1], f[2])?;
    }
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{Mesh, stats};

    /// A tetrahedron facing out, its edges stored in both directions.
    pub(crate) const TETRA: &str = "# a tetrahedron\n4 6 4 GtsSurface GtsFace GtsEdge GtsVertex\n\
        0 0 0\n10 0 0\n0 10 0\n0 0 10\n\
        1 3\n3 2\n2 1\n2 4\n4 1\n3 4\n\
        1 2 3\n3 4 5\n5 6 1\n2 6 4\n";

    #[test]
    fn reads_a_tetrahedron_facing_out() {
        let loaded = read(TETRA.as_bytes(), &mut Progress::silent()).unwrap();
        let m = &loaded.objects[0].mesh;
        assert_eq!((m.triangle_count(), m.vertex_count()), (4, 4));
        let s = stats(m);
        assert!((s.volume - 1000.0 / 6.0).abs() < 1e-9, "{}", s.volume);
        assert_eq!(s.open_edges, 0);
        // Swapping a triangle's first two edges turns it round, as the GTS library does.
        assert_eq!(corners([0, 2], [2, 1]), Some([0, 2, 1]));
        assert_eq!(corners([2, 1], [0, 2]), Some([1, 2, 0]));
    }

    #[test]
    fn round_trip_keeps_the_triangles_and_the_way_they_face() {
        let mesh = Mesh {
            positions: vec![
                0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 0.0, 10.0,
            ],
            indices: vec![0, 2, 1, 0, 1, 3, 1, 2, 3, 0, 3, 2],
        };
        let parts = [NamedMesh {
            name: "t",
            mesh: &mesh,
        }];
        let bytes = write(Vec::new(), &parts).unwrap();
        assert!(String::from_utf8_lossy(&bytes).starts_with("4 6 4 "));
        let back = read(&bytes, &mut Progress::silent()).unwrap();
        // Reading welds, which numbers the points in the order the triangles first use them.
        assert_eq!(
            back.objects[0].mesh,
            weld_indexed(&mesh.positions, &mesh.indices).unwrap()
        );
    }

    #[test]
    fn errors_name_what_is_wrong() {
        assert!(read(b"solid x", &mut Progress::silent()).is_err());
        assert_eq!(
            read(
                b"3 3 1\n0 0 0\n1 0 0\n0 1 0\n1 2\n2 3\n3 9\n1 2 3\n",
                &mut Progress::silent()
            )
            .unwrap_err(),
            "its GTS data is not valid at edge 3"
        );
        assert_eq!(
            read(
                b"3 3 2\n0 0 0\n1 0 0\n0 1 0\n1 2\n2 3\n3 1\n1 2 3\n",
                &mut Progress::silent()
            )
            .unwrap_err(),
            "the file ends early (expected 2 triangles, found 1)"
        );
    }
}
