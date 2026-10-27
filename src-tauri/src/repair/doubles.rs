//! Remove double triangles: delete triangles that repeat another triangle's three corners, in any
//! order or orientation. The first of each group stays. With a selection, only selected copies go.

use super::topology::keep_triangles;
use super::{Control, Origin, Outcome, RepairResult, silent};
use crate::mesh::{Mesh, Welder, fmt_count};

/// Vertex buckets between cancel checks.
const CHECK_EVERY: usize = 1 << 16;

/// Deletes every triangle whose three corners repeat an earlier triangle's, in any order or
/// orientation, so the first of each group stays. Corners are the same when their coordinates are
/// equal, so copies with vertices of their own count too. With a selection only selected triangles
/// go: a group with an unselected member keeps its unselected members and loses its selected ones,
/// and a group selected throughout keeps its first. The remaining triangles keep their order, and
/// vertices only removed triangles used are dropped.
pub fn run(mesh: &Mesh, selection: &[u32], control: &mut Control) -> RepairResult<Outcome> {
    control.check()?;
    let first = first_copies(mesh, control)?;
    control.progress(0.8);
    let keep: Vec<bool> = match selected_mask(mesh, selection) {
        None => first
            .iter()
            .enumerate()
            .map(|(t, &f)| f as usize == t)
            .collect(),
        Some(selected) => {
            // Per group, at its first triangle: whether some member is not selected.
            let mut open = vec![false; first.len()];
            for (t, &f) in first.iter().enumerate() {
                if !selected[t] {
                    open[f as usize] = true;
                }
            }
            first
                .iter()
                .enumerate()
                .map(|(t, &f)| {
                    if open[f as usize] {
                        !selected[t]
                    } else {
                        f as usize == t
                    }
                })
                .collect()
        }
    };
    let removed = keep.iter().filter(|&&k| !k).count();
    if removed == 0 {
        return Ok(Outcome::unchanged(mesh, "No double triangles to remove."));
    }
    let (out, kept) = keep_triangles(mesh, &keep);
    control.progress(1.0);
    let unit = if removed == 1 {
        "double triangle"
    } else {
        "double triangles"
    };
    Ok(Outcome {
        mesh: out,
        changed: true,
        origin: Origin::Map(kept),
        message: format!("Removed {} {unit}.", fmt_count(removed as u64)),
        warnings: Vec::new(),
        highlight: None,
        select: None,
        concerns: Vec::new(),
    })
}

/// The mesh without double triangles (first of each group kept, triangle order kept), its origin
/// map, and how many went. Corners count as the same when their coordinates are equal.
pub fn remove_doubles(mesh: &Mesh) -> (Mesh, Vec<u32>, usize) {
    let count = mesh.triangle_count();
    // A silent control never cancels; if it did, nothing would count as a copy.
    let first = silent(|control| first_copies(mesh, control))
        .unwrap_or_else(|_| (0..count as u32).collect());
    let keep: Vec<bool> = first
        .iter()
        .enumerate()
        .map(|(t, &f)| f as usize == t)
        .collect();
    let removed = keep.iter().filter(|&&k| !k).count();
    if removed == 0 {
        return (mesh.clone(), (0..count as u32).collect(), 0);
    }
    let (out, origin) = keep_triangles(mesh, &keep);
    (out, origin, removed)
}

/// For each triangle, the first triangle over the same three corners: itself when no earlier one
/// is. Corners are the same when their coordinates are equal (-0.0 counts as +0.0).
///
/// Counting sort by each triangle's smallest corner, then a small stable sort per bucket: linear
/// time and no hash map of triples.
fn first_copies(mesh: &Mesh, control: &Control) -> RepairResult<Vec<u32>> {
    let same = first_at_position(mesh);
    control.check()?;
    let keys: Vec<[u32; 3]> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| sorted(t.map(|v| same[v as usize])))
        .collect();
    drop(same);
    let count = mesh.vertex_count();
    let mut start = vec![0u32; count + 1];
    for k in &keys {
        start[k[0] as usize + 1] += 1;
    }
    for v in 0..count {
        start[v + 1] += start[v];
    }
    let mut fill = start[..count].to_vec();
    let mut order = vec![0u32; keys.len()];
    for (t, k) in keys.iter().enumerate() {
        order[fill[k[0] as usize] as usize] = t as u32;
        fill[k[0] as usize] += 1;
    }
    drop(fill);
    control.check()?;
    let mut first: Vec<u32> = (0..keys.len() as u32).collect();
    for v in 0..count {
        if v % CHECK_EVERY == 0 {
            control.check()?;
        }
        let bucket = &mut order[start[v] as usize..start[v + 1] as usize];
        if bucket.len() < 2 {
            continue;
        }
        // The triangles went in ascending, and the sort is stable, so each group of equal corners
        // starts with its first triangle.
        bucket.sort_by_key(|&t| {
            let k = keys[t as usize];
            (k[1], k[2])
        });
        for group in bucket.chunk_by(|&a, &b| keys[a as usize] == keys[b as usize]) {
            for &t in &group[1..] {
                first[t as usize] = group[0];
            }
        }
    }
    Ok(first)
}

/// For each vertex, the first vertex at exactly its position (-0.0 counts as +0.0). The same as
/// sanitize's exact weld, which is private there.
fn first_at_position(mesh: &Mesh) -> Vec<u32> {
    // Welder hands out ids in order of first appearance.
    let mut welder = Welder::new(mesh.vertex_count() * 5);
    let mut first: Vec<u32> = Vec::with_capacity(mesh.vertex_count());
    (0..mesh.vertex_count() as u32)
        .map(|v| {
            let id = welder.vertex(mesh.vertex(v)) as usize;
            if id == first.len() {
                first.push(v);
            }
            first[id]
        })
        .collect()
}

/// The three values in ascending order.
fn sorted([a, b, c]: [u32; 3]) -> [u32; 3] {
    let (a, b) = (a.min(b), a.max(b));
    let (b, c) = (b.min(c), b.max(c));
    [a.min(b), a.max(b), c]
}

/// One flag per triangle for the selected ones, or None when the selection is empty. Ids past the
/// last triangle are ignored.
fn selected_mask(mesh: &Mesh, selection: &[u32]) -> Option<Vec<bool>> {
    if selection.is_empty() {
        return None;
    }
    let mut mask = vec![false; mesh.triangle_count()];
    for &t in selection {
        if let Some(slot) = mask.get_mut(t as usize) {
            *slot = true;
        }
    }
    Some(mask)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    use super::*;
    use crate::mesh::{make_box, stats};
    use crate::ops::concat;
    use crate::repair::{RepairError, remap_selection};

    fn cube() -> Mesh {
        make_box([0.0; 3], [20.0; 3])
    }

    fn remove(mesh: &Mesh, selection: &[u32]) -> Outcome {
        silent(|c| run(mesh, selection, c)).unwrap()
    }

    /// The cube with triangle `t` added again at the end, its corners rotated by `rotate` and
    /// turned round when `flip` is set.
    fn with_copy(mut mesh: Mesh, t: usize, rotate: usize, flip: bool) -> Mesh {
        let mut c = [0, 1, 2].map(|k| mesh.indices[t * 3 + k]);
        c.rotate_left(rotate);
        if flip {
            c.swap(1, 2);
        }
        mesh.indices.extend_from_slice(&c);
        mesh
    }

    fn closed_and_oriented(mesh: &Mesh) -> bool {
        let s = stats(mesh);
        s.open_edges == 0 && s.non_manifold_edges == 0 && s.volume > 0.0
    }

    #[test]
    fn sorted_orders_every_permutation() {
        for t in [
            [1, 2, 3],
            [1, 3, 2],
            [2, 1, 3],
            [2, 3, 1],
            [3, 1, 2],
            [3, 2, 1],
        ] {
            assert_eq!(sorted(t), [1, 2, 3]);
        }
        assert_eq!(sorted([5, 5, 1]), [1, 5, 5]);
    }

    #[test]
    fn a_clean_mesh_is_unchanged() {
        let mesh = cube();
        let out = remove(&mesh, &[]);
        assert!(!out.changed);
        assert_eq!(out.origin, Origin::Same);
        assert_eq!(out.mesh, mesh);
        assert_eq!(out.message, "No double triangles to remove.");
        let (same, origin, removed) = remove_doubles(&mesh);
        assert_eq!((same, removed), (mesh, 0));
        assert_eq!(origin, (0..12).collect::<Vec<u32>>());
        let empty = Mesh::default();
        assert!(!remove(&empty, &[]).changed);
        assert_eq!(remove_doubles(&empty), (empty, Vec::new(), 0));
    }

    #[test]
    fn a_copy_goes_whatever_its_corner_order_and_orientation() {
        for rotate in 0..3 {
            for flip in [false, true] {
                let mesh = with_copy(cube(), 4, rotate, flip);
                assert!(!closed_and_oriented(&mesh));
                let out = remove(&mesh, &[]);
                assert!(out.changed);
                assert_eq!(out.message, "Removed 1 double triangle.");
                assert_eq!(out.mesh, cube(), "rotate {rotate} flip {flip}");
                assert_eq!(out.origin, Origin::Map((0..12).collect()));
                assert!(closed_and_oriented(&out.mesh));
                assert!(!remove(&out.mesh, &[]).changed, "idempotent");
            }
        }
    }

    #[test]
    fn the_first_of_a_group_stays_even_when_it_is_the_odd_one() {
        // Triangle 0 turned round, then two copies of the original at the end: the turned triangle
        // comes first, so it stays and both originals go, whatever their winding.
        let mut mesh = cube();
        mesh.indices.swap(1, 2);
        let first = [mesh.indices[0], mesh.indices[1], mesh.indices[2]];
        let mesh = with_copy(with_copy(mesh, 0, 1, true), 0, 2, true);
        assert_eq!(mesh.triangle_count(), 14);
        let out = remove(&mesh, &[]);
        assert_eq!(out.message, "Removed 2 double triangles.");
        assert_eq!(out.origin, Origin::Map((0..12).collect()));
        assert_eq!(out.mesh.indices[0..3], first);
        let (m, origin, removed) = remove_doubles(&mesh);
        assert_eq!((m, origin, removed), (out.mesh, (0..12).collect(), 2));
    }

    #[test]
    fn copies_with_their_own_vertices_count_and_their_vertices_go() {
        // The +z face again, with new vertices at the same coordinates, one of them a -0.0 where
        // the original has +0.0.
        let mut mesh = make_box([0.0; 3], [20.0; 3]);
        let face: Vec<u32> = mesh.indices[6..12].to_vec();
        let base = mesh.vertex_count() as u32;
        for (n, &v) in face.iter().enumerate() {
            let mut p = mesh.vertex(v);
            if p[0] == 0.0 {
                p[0] = -0.0;
            }
            mesh.positions.extend_from_slice(&p);
            mesh.indices.push(base + n as u32);
        }
        let out = remove(&mesh, &[]);
        assert_eq!(out.message, "Removed 2 double triangles.");
        assert_eq!(out.mesh, cube());
        // A copy with a corner a hair away is a different triangle.
        let mut near = mesh.clone();
        near.positions[base as usize * 3] = f32::from_bits(1);
        let out = remove(&near, &[]);
        assert_eq!(out.message, "Removed 1 double triangle.");
        assert_eq!(out.mesh.triangle_count(), 13);
    }

    #[test]
    fn triangles_sharing_two_corners_are_not_doubles() {
        // Three fins on one edge and a fourth triangle repeating one of them.
        let positions = vec![
            0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 0.0,
        ];
        let mesh = Mesh {
            positions,
            indices: vec![0, 1, 2, 0, 1, 3, 1, 0, 4, 3, 0, 1],
        };
        let out = remove(&mesh, &[]);
        assert_eq!(out.origin, Origin::Map(vec![0, 1, 2]));
        // Degenerate triangles over the same corners are doubles too.
        let flat = Mesh {
            positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            indices: vec![0, 0, 1, 0, 1, 0, 1, 1, 0],
        };
        let out = remove(&flat, &[]);
        assert_eq!(out.origin, Origin::Map(vec![0, 2]));
    }

    #[test]
    fn with_a_selection_only_selected_copies_go() {
        // Triangle 12 repeats triangle 4, and triangle 13 repeats it again.
        let mesh = with_copy(with_copy(cube(), 4, 1, false), 4, 2, true);
        let keep_of = |selection: &[u32]| -> Vec<u32> {
            match remove(&mesh, selection).origin {
                Origin::Map(map) => map,
                Origin::Same => (0..mesh.triangle_count() as u32).collect(),
            }
        };
        let all: Vec<u32> = (0..14).collect();
        let without = |gone: &[u32]| -> Vec<u32> {
            all.iter().copied().filter(|t| !gone.contains(t)).collect()
        };
        // A selected copy goes; the unselected ones stay, the other copy included.
        assert_eq!(keep_of(&[12]), without(&[12]));
        // The first one selected: it goes, and both unselected copies stay.
        assert_eq!(keep_of(&[4]), without(&[4]));
        assert_eq!(keep_of(&[4, 13]), without(&[4, 13]));
        // The whole group selected: its first stays.
        assert_eq!(keep_of(&[4, 12, 13]), without(&[12, 13]));
        assert_eq!(keep_of(&all), without(&[12, 13]));
        // A selection elsewhere removes nothing, and so does one of ids past the end.
        let out = remove(&mesh, &[0, 1, 99]);
        assert!(!out.changed);
        assert_eq!(out.mesh, mesh);
        assert!(!remove(&mesh, &[99]).changed);
        // The selection follows the kept triangles.
        let out = remove(&mesh, &[4, 12, 13]);
        assert_eq!(
            remap_selection(&[4, 12, 13], &out.origin, out.mesh.triangle_count()),
            vec![4]
        );
    }

    #[test]
    fn a_whole_copied_shell_goes_whatever_its_orientation() {
        // A second cube with vertices of its own over the first: all of it goes, facing out or in.
        for flip in [false, true] {
            let mut copy = cube();
            if flip {
                for t in 0..12 {
                    copy.indices.swap(t * 3 + 1, t * 3 + 2);
                }
            }
            let mesh = concat(&[&cube(), &copy]).unwrap();
            let out = remove(&mesh, &[]);
            assert_eq!(out.message, "Removed 12 double triangles.");
            assert_eq!(out.mesh, cube());
        }
        // A cube beside it sharing one face's position only: its triangles there run along the
        // other diagonal, so nothing repeats.
        let beside = concat(&[&cube(), &make_box([20.0, 0.0, 0.0], [40.0, 20.0, 20.0])]).unwrap();
        assert!(!remove(&beside, &[]).changed);
    }

    #[test]
    fn of_the_fixtures_only_the_double_triangle_changes() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repair");
        let mut changed = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "stl") {
                continue;
            }
            let imported =
                crate::formats::import_file(&path, true, &mut crate::progress::Progress::silent())
                    .unwrap();
            let out = remove(&imported.parts[0].mesh, &[]);
            if out.changed {
                assert_eq!(out.message, "Removed 1 double triangle.");
                assert!(closed_and_oriented(&out.mesh));
                assert!(!remove(&out.mesh, &[]).changed);
                changed.push(path.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
        assert_eq!(changed, vec!["cube-double-triangle.stl"]);
    }

    #[test]
    fn a_cancelled_run_stops_and_progress_reaches_the_end() {
        let mesh = with_copy(cube(), 3, 0, false);
        let cancel = AtomicBool::new(true);
        let mut control = Control::new(&cancel, |_| {});
        assert_eq!(run(&mesh, &[], &mut control), Err(RepairError::Cancelled));
        let never = AtomicBool::new(false);
        let mut seen = Vec::new();
        {
            let mut control = Control::new(&never, |f| seen.push(f));
            run(&mesh, &[], &mut control).unwrap();
        }
        assert!(seen.is_sorted() && seen.last() == Some(&1.0));
    }

    /// A closed torus of `n` × `n` quads (2n² triangles).
    fn torus(n: u32) -> Mesh {
        crate::repair::exact::testing::torus(40.0, 15.0, n as usize, n as usize)
    }

    #[test]
    fn many_copies_in_a_big_mesh_are_all_found() {
        let base = torus(60);
        let mut mesh = base.clone();
        let mut want = 0;
        for t in (0..base.triangle_count()).step_by(7) {
            mesh = with_copy(mesh, t, t % 3, t % 2 == 0);
            want += 1;
        }
        let out = remove(&mesh, &[]);
        assert_eq!(out.mesh, base);
        assert_eq!(
            out.message,
            format!("Removed {} double triangles.", fmt_count(want))
        );
    }

    /// `cargo test --release --lib repair::doubles -- --ignored --nocapture`: about 730k triangles,
    /// clean and with 1% doubles.
    #[test]
    #[ignore]
    fn timing_on_730k_triangles() {
        let clean = torus(604);
        let mut doubled = clean.clone();
        for t in (0..clean.triangle_count()).step_by(100) {
            let c = [0, 1, 2].map(|k| clean.indices[t * 3 + k]);
            doubled.indices.extend_from_slice(&[c[0], c[2], c[1]]);
        }
        for (name, mesh) in [("clean", &clean), ("1% doubles", &doubled)] {
            let started = Instant::now();
            let out = remove(mesh, &[]);
            let took = started.elapsed().as_secs_f64();
            println!(
                "remove doubles, {name}: {} triangles in {:.0} ms: {}",
                mesh.triangle_count(),
                took * 1000.0,
                out.message
            );
            let started = Instant::now();
            let (_, _, removed) = remove_doubles(mesh);
            println!(
                "remove_doubles, {name}: {:.0} ms, {removed} removed",
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
        // Real models: TRICKY3D_MODELS="a.stl;b.stl".
        let Ok(list) = std::env::var("TRICKY3D_MODELS") else {
            return;
        };
        for path in list.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let imported = crate::formats::import_file(
                std::path::Path::new(path),
                true,
                &mut crate::progress::Progress::silent(),
            )
            .unwrap();
            for part in &imported.parts {
                let started = Instant::now();
                let out = remove(&part.mesh, &[]);
                println!(
                    "{path}: {} triangles in {:.0} ms: {}",
                    part.mesh.triangle_count(),
                    started.elapsed().as_secs_f64() * 1000.0,
                    out.message
                );
            }
        }
    }
}
