//! Home › Repair selections that need the whole mesh: Select by criteria (every damaged triangle)
//! and Find overlapping triangles. They moved here from the window (src/repair/edits.ts in PR #1)
//! and select the same triangles. Neither changes the mesh; each replaces the selection when it
//! finds something and leaves it alone when not.

use std::f64::consts::PI;

use super::defaults::{DEGENERATE, effective_degenerate_tolerance};
use super::orient;
use super::status::degenerate_triangles;
use super::topology::{Edges, orientation_patches};
use super::{Control, Outcome, RepairResult, tritri};
use crate::mesh::{Mesh, fmt_count};

/// Corners a triangle's angle must stay below for the triangle not to count as slim: 1°.
const SLIM_DEGREES: f64 = 1.0;

/// Selects every damaged triangle: one on an open or non-manifold edge, one with an angle under
/// 1° or a zero-length edge (as the viewport's slim highlight at 1°), and the inverted and
/// degenerate triangles the Status tab highlights.
pub fn problem_triangles(mesh: &Mesh, control: &mut Control) -> RepairResult<Outcome> {
    control.check()?;
    let count = mesh.triangle_count();
    let mut mark = vec![false; count];
    let edges = Edges::new(mesh);
    for e in (0..edges.len() as u32).filter(|&e| edges.degree(e) != 2) {
        for &side in edges.sides(e) {
            mark[side as usize / 3] = true;
        }
    }
    control.progress(0.2);
    control.check()?;
    let limit = (SLIM_DEGREES * PI / 180.0).cos();
    for (t, m) in mark.iter_mut().enumerate() {
        if slim(mesh, t, limit) {
            *m = true;
        }
    }
    control.progress(0.3);
    control.check()?;
    let patches = orientation_patches(mesh, &edges);
    let inverted = orient::inverted(
        mesh,
        &edges,
        &patches,
        orient::Nested::AsMost,
        &mut control.sub(0.3, 0.9),
    )?;
    for (m, &flipped) in mark.iter_mut().zip(&inverted) {
        *m |= flipped;
    }
    for t in degenerate_triangles(mesh, effective_degenerate_tolerance(&DEGENERATE)) {
        mark[t as usize] = true;
    }
    control.progress(1.0);
    let found: Vec<u32> = (0..count as u32).filter(|&t| mark[t as usize]).collect();
    if found.is_empty() {
        return Ok(Outcome::unchanged(mesh, "No problem triangles found."));
    }
    let n = found.len();
    let unit = if n == 1 { "triangle" } else { "triangles" };
    let mut out = Outcome::unchanged(
        mesh,
        format!("Selected {} problem {unit}.", fmt_count(n as u64)),
    );
    out.select = Some(found);
    Ok(out)
}

/// Selects the triangles that cross another one (Detect self-intersections over the whole part,
/// whose highlight it also leaves) and those that repeat another triangle's three vertices.
pub fn overlaps(mesh: &Mesh, control: &mut Control) -> RepairResult<Outcome> {
    control.check()?;
    let crossing = tritri::intersecting_triangles(mesh, &mut control.sub(0.0, 0.9))?;
    let mut mark = vec![false; mesh.triangle_count()];
    for &t in &crossing {
        mark[t as usize] = true;
    }
    for t in repeated_triangles(mesh) {
        mark[t as usize] = true;
    }
    control.progress(1.0);
    let found: Vec<u32> = (0..mark.len() as u32)
        .filter(|&t| mark[t as usize])
        .collect();
    let mut out = if found.is_empty() {
        Outcome::unchanged(mesh, "No overlapping triangles found.")
    } else {
        let n = found.len();
        let unit = if n == 1 { "triangle" } else { "triangles" };
        let mut out = Outcome::unchanged(
            mesh,
            format!("Found {} overlapping {unit}.", fmt_count(n as u64)),
        );
        out.select = Some(found);
        out
    };
    out.highlight = Some(crossing);
    Ok(out)
}

/// Whether triangle `t` has a zero-length edge or an angle whose cosine is above `limit`, measured
/// as src/viewport/repairLayer.ts `slimMask` measures it.
fn slim(mesh: &Mesh, t: usize, limit: f64) -> bool {
    let c = &mesh.indices[t * 3..t * 3 + 3];
    let [a, b, d] = [0, 1, 2].map(|k| mesh.vertex(c[k]).map(f64::from));
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ac = [d[0] - a[0], d[1] - a[1], d[2] - a[2]];
    let bc = [d[0] - b[0], d[1] - b[1], d[2] - b[2]];
    let sq = |v: [f64; 3]| v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    let dot = |u: [f64; 3], v: [f64; 3]| u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
    let (lab, lac, lbc) = (sq(ab), sq(ac), sq(bc));
    if lab == 0.0 || lac == 0.0 || lbc == 0.0 {
        return true;
    }
    let at_a = dot(ab, ac) / (lab * lac).sqrt();
    let at_b = -dot(ab, bc) / (lab * lbc).sqrt();
    let at_c = dot(ac, bc) / (lac * lbc).sqrt();
    at_a > limit || at_b > limit || at_c > limit
}

/// Every triangle over the same three vertex indices as another one, in any order. Sorted.
fn repeated_triangles(mesh: &Mesh) -> Vec<u32> {
    let mut keys: Vec<([u32; 3], u32)> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .enumerate()
        .map(|(t, &c)| {
            let mut k = c;
            k.sort_unstable();
            (k, t as u32)
        })
        .collect();
    keys.sort_unstable();
    let mut out: Vec<u32> = keys
        .chunk_by(|a, b| a.0 == b.0)
        .filter(|group| group.len() > 1)
        .flat_map(|group| group.iter().map(|&(_, t)| t))
        .collect();
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::mesh::make_box;
    use crate::ops::concat;
    use crate::repair::{RepairError, silent};

    fn tri(positions: &[f32], indices: &[u32]) -> Mesh {
        Mesh {
            positions: positions.to_vec(),
            indices: indices.to_vec(),
        }
    }

    fn cube() -> Mesh {
        make_box([0.0; 3], [20.0; 3])
    }

    #[test]
    fn an_open_face_is_a_problem_and_a_closed_cube_has_none() {
        let face = tri(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0], &[0, 1, 2]);
        let out = silent(|c| problem_triangles(&face, c)).unwrap();
        assert!(!out.changed);
        assert_eq!(out.select, Some(vec![0]));
        assert_eq!(out.message, "Selected 1 problem triangle.");
        let none = silent(|c| problem_triangles(&cube(), c)).unwrap();
        assert_eq!(
            none.select, None,
            "nothing found leaves the selection alone"
        );
        assert_eq!(none.message, "No problem triangles found.");
    }

    #[test]
    fn inverted_slim_and_degenerate_triangles_are_problems() {
        // One triangle of the cube turned round: it is the inverted one.
        let mut turned = cube();
        turned.indices.swap(4 * 3 + 1, 4 * 3 + 2);
        let out = silent(|c| problem_triangles(&turned, c)).unwrap();
        assert_eq!(out.select, Some(vec![4]));
        // A closed tetrahedron with a 0.087 mm edge: the two faces that meet the far corner across
        // it have a corner of about 0.5° there and are slim, though nothing else is wrong.
        let thin = tri(
            &[
                0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 10.0, 0.0873, 0.0, 5.0, 0.0, 8.0,
            ],
            &[0, 2, 1, 0, 1, 3, 1, 2, 3, 2, 0, 3],
        );
        let out = silent(|c| problem_triangles(&thin, c)).unwrap();
        assert_eq!(out.select, Some(vec![0, 2]));
        let limit = (SLIM_DEGREES * PI / 180.0).cos();
        assert!(slim(&thin, 0, limit) && !slim(&cube(), 0, limit));
        // Three corners on a line: slim and degenerate, and its edges are open.
        let flat = tri(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 2.0, 0.0, 0.0], &[0, 1, 2]);
        assert_eq!(
            silent(|c| problem_triangles(&flat, c)).unwrap().select,
            Some(vec![0])
        );
    }

    #[test]
    fn overlaps_are_crossing_and_repeated_triangles() {
        let doubles = tri(
            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            &[0, 1, 2, 2, 1, 0, 0, 2, 1],
        );
        let out = silent(|c| overlaps(&doubles, c)).unwrap();
        assert!(!out.changed);
        assert_eq!(out.select, Some(vec![0, 1, 2]));
        assert_eq!(out.message, "Found 3 overlapping triangles.");
        // Two cubes through each other: the crossing triangles are both selected and highlighted.
        let two = concat(&[&cube(), &make_box([10.0, 0.0, 0.0], [30.0, 20.0, 20.0])]).unwrap();
        let out = silent(|c| overlaps(&two, c)).unwrap();
        let crossing = out.highlight.clone().unwrap();
        assert!(!crossing.is_empty());
        assert_eq!(out.select, Some(crossing));
        let clean = silent(|c| overlaps(&cube(), c)).unwrap();
        assert_eq!(clean.select, None);
        assert_eq!(clean.highlight, Some(Vec::new()));
        assert_eq!(clean.message, "No overlapping triangles found.");
    }

    #[test]
    fn a_cancelled_search_stops() {
        let cancel = AtomicBool::new(true);
        let mut control = Control::new(&cancel, |_| {});
        assert_eq!(
            problem_triangles(&cube(), &mut control),
            Err(RepairError::Cancelled)
        );
        assert_eq!(overlaps(&cube(), &mut control), Err(RepairError::Cancelled));
    }
}
