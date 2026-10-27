//! Sanitize duplicate nodes: merge vertices that lie closer together than the tolerance, then drop
//! the triangles that collapse (two corners merged). Ignores the selection.

use std::borrow::Cow;
use std::collections::HashMap;

use super::topology::keep_triangles;
use super::{Control, Origin, Outcome, RepairResult, SanitizeParams};
use crate::mesh::{Mesh, Welder, fmt_count};
use crate::ops::compact;

/// Merges every vertex into the nearest earlier vertex within `params.tolerance` mm (the earlier
/// one keeps its position, so nothing moves by more than the tolerance and merges never chain),
/// then removes the triangles left with a repeated corner and the vertices nothing uses. Vertices
/// no triangle uses take no part, so they can never pull a corner onto their position.
pub fn run(
    mesh: &Mesh,
    selection: &[u32],
    params: &SanitizeParams,
    control: &mut Control,
) -> RepairResult<Outcome> {
    let _ = selection;
    control.check()?;
    let mut used = vec![false; mesh.vertex_count()];
    for &i in &mesh.indices {
        used[i as usize] = true;
    }
    let unused = used.iter().filter(|&&u| !u).count();
    drop(used);
    let base: Cow<Mesh> = if unused > 0 {
        Cow::Owned(compact(&mesh.positions, mesh.indices.clone()))
    } else {
        Cow::Borrowed(mesh)
    };
    control.progress(0.1);
    let into = merge_targets(&base, params.tolerance);
    control.check()?;
    control.progress(0.7);
    let merged = into
        .iter()
        .enumerate()
        .filter(|&(v, &target)| target as usize != v)
        .count();
    let welded = Mesh {
        positions: base.positions.clone(),
        indices: base.indices.iter().map(|&i| into[i as usize]).collect(),
    };
    let keep: Vec<bool> = welded
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| t[0] != t[1] && t[1] != t[2] && t[2] != t[0])
        .collect();
    let removed = keep.iter().filter(|&&k| !k).count();
    if merged == 0 && removed == 0 && unused == 0 {
        return Ok(Outcome::unchanged(mesh, "No duplicate nodes to merge."));
    }
    let (out, kept) = keep_triangles(&welded, &keep);
    control.progress(1.0);
    Ok(Outcome {
        mesh: out,
        changed: true,
        // Merging alone keeps every triangle where it was.
        origin: if removed == 0 {
            Origin::Same
        } else {
            Origin::Map(kept)
        },
        message: report(merged, removed, unused),
        warnings: Vec::new(),
        highlight: None,
        select: None,
        concerns: Vec::new(),
    })
}

/// weld_within's grid keys are `floor(coordinate / (4 × tolerance))` as i64; below this ratio they
/// stay exact and far from overflowing.
const GRID_LIMIT: f64 = (1u64 << 62) as f64;

/// For each vertex, the vertex it merges into (itself when none): weld_within, or an exact weld
/// when the tolerance is so small against the part's coordinates that weld_within's grid keys
/// would overflow (a tolerance of 0, say), which would put every vertex into one cell. f32
/// coordinates that close are equal anyway, except within about 1e-12 of the part's size of zero.
fn merge_targets(mesh: &Mesh, tolerance: f64) -> Vec<u32> {
    let largest = mesh
        .positions
        .iter()
        .fold(0f64, |m, &v| m.max((v as f64).abs()));
    // NaN (a tolerance of 0 with every vertex at the origin) and infinity both fail the test.
    if (largest + tolerance) / (4.0 * tolerance) < GRID_LIMIT {
        weld_within(mesh, tolerance)
    } else {
        exact_targets(mesh)
    }
}

const NONE: u32 = u32::MAX;

/// For each vertex, the vertex it welds to: the nearest earlier kept vertex within `tolerance`, or
/// itself. The first position wins (no averaging), so the result does not depend on how many
/// copies there were.
///
/// A uniform grid hash with cells of 4 × tolerance: the ball around a vertex then overlaps 1 to 8
/// cells instead of the 27 a tolerance-sized grid needs, and the answer is the same.
fn weld_within(mesh: &Mesh, tolerance: f64) -> Vec<u32> {
    let count = mesh.vertex_count();
    let cell = (tolerance * 4.0).max(f64::MIN_POSITIVE);
    let limit = tolerance * tolerance;
    // Kept vertices per cell as linked lists: the head per cell, then `next` per vertex.
    let mut heads: HashMap<[i64; 3], u32> = HashMap::with_capacity(count);
    let mut next = vec![NONE; count];
    let mut remap = vec![NONE; count];
    let key = |x: f64| (x / cell).floor() as i64;
    for v in 0..count as u32 {
        let p = mesh.point(v);
        let lo = p.map(|x| key(x - tolerance));
        let hi = p.map(|x| key(x + tolerance));
        let mut best = (f64::INFINITY, NONE);
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    let mut k = heads.get(&[x, y, z]).copied().unwrap_or(NONE);
                    while k != NONE {
                        let q = mesh.point(k);
                        let d =
                            (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2);
                        // Ties go to the lower id, so the cell visiting order does not matter.
                        if d <= limit && (d < best.0 || (d == best.0 && k < best.1)) {
                            best = (d, k);
                        }
                        k = next[k as usize];
                    }
                }
            }
        }
        if best.1 != NONE {
            remap[v as usize] = best.1;
        } else {
            remap[v as usize] = v;
            let head = heads.entry(p.map(key)).or_insert(NONE);
            next[v as usize] = *head;
            *head = v;
        }
    }
    remap
}

/// For each vertex, the first vertex at exactly the same position (-0.0 counts as +0.0).
fn exact_targets(mesh: &Mesh) -> Vec<u32> {
    // Welder's hash table hands out ids in order of first appearance.
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

/// "Merged 12 nodes, removed 3 degenerate triangles." with the parts that are zero left out.
fn report(merged: usize, removed: usize, unused: usize) -> String {
    let mut parts = Vec::new();
    if merged > 0 {
        parts.push(format!("merged {}", count(merged, "node", "nodes")));
    }
    if removed > 0 {
        parts.push(format!(
            "removed {}",
            count(removed, "degenerate triangle", "degenerate triangles")
        ));
    }
    if unused > 0 {
        parts.push(format!(
            "dropped {}",
            count(unused, "unused node", "unused nodes")
        ));
    }
    let text = parts.join(", ");
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

/// "1 node", "1,204 nodes".
fn count(n: usize, one: &str, many: &str) -> String {
    format!(
        "{} {}",
        fmt_count(n as u64),
        if n == 1 { one } else { many }
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::mesh::{make_box, stats};
    use crate::repair::{Control, NEW, RepairError, remap_selection, silent};

    const TOLERANCE: SanitizeParams = SanitizeParams { tolerance: 0.0001 };

    fn sanitize(mesh: &Mesh, tolerance: f64) -> Outcome {
        silent(|c| run(mesh, &[], &SanitizeParams { tolerance }, c)).unwrap()
    }

    #[test]
    fn welds_within_tolerance_to_the_first_position() {
        // Three triangles whose shared corners are a little apart; a fourth vertex is too far.
        let positions = vec![
            0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, //
            0.00005, 0.0, 0.0, 0.0, 1.00008, 0.0, 0.0, 0.0, 1.0, //
            0.0003, 0.0, 0.0,
        ];
        let mesh = Mesh {
            positions,
            indices: vec![0, 1, 2, 3, 4, 5, 6, 1, 2],
        };
        let remap = weld_within(&mesh, TOLERANCE.tolerance);
        assert_eq!(remap, vec![0, 1, 2, 0, 2, 5, 6]);
    }

    #[test]
    fn weld_picks_the_nearest_kept_vertex() {
        let positions = vec![0.0, 0.0, 0.0, 0.00015, 0.0, 0.0, 0.0001, 0.0, 0.0];
        let mesh = Mesh {
            positions,
            indices: vec![],
        };
        // Vertex 2 is within tolerance of both kept vertices and nearer to vertex 1.
        assert_eq!(weld_within(&mesh, TOLERANCE.tolerance), vec![0, 1, 1]);
    }

    /// The cube with every triangle given its own three vertices (a triangle soup, as an STL
    /// reader that does not weld would give).
    fn soup(mesh: &Mesh) -> Mesh {
        let mut positions = Vec::new();
        for &i in &mesh.indices {
            positions.extend_from_slice(&mesh.vertex(i));
        }
        let indices = (0..mesh.indices.len() as u32).collect();
        Mesh { positions, indices }
    }

    #[test]
    fn a_clean_mesh_is_unchanged() {
        let cube = make_box([0.0; 3], [20.0; 3]);
        let out = sanitize(&cube, 0.0001);
        assert!(!out.changed);
        assert_eq!(out.origin, Origin::Same);
        assert_eq!(out.mesh, cube);
        assert_eq!(out.message, "No duplicate nodes to merge.");
    }

    #[test]
    fn a_triangle_soup_welds_back_into_the_closed_cube() {
        let cube = make_box([0.0; 3], [20.0; 3]);
        let out = sanitize(&soup(&cube), 0.0001);
        assert!(out.changed);
        // Nothing collapsed, so every triangle stays where it was.
        assert_eq!(out.origin, Origin::Same);
        assert_eq!(out.mesh.vertex_count(), 8);
        assert_eq!(out.message, "Merged 28 nodes.");
        let s = stats(&out.mesh);
        assert_eq!((s.open_edges, s.non_manifold_edges, s.shells), (0, 0, 1));
        assert_eq!(s.volume, 8000.0);
        // Running it again changes nothing.
        assert!(!sanitize(&out.mesh, 0.0001).changed);
    }

    #[test]
    fn near_corners_merge_into_the_first_and_collapsed_triangles_go() {
        // Triangle 0 is fine; triangle 1 has two corners 0.00005 apart, which merge, so it
        // collapses; triangle 2 shares a corner with triangle 0 only within the tolerance.
        let positions = vec![
            0.0, 0.0, 0.0, // 0
            1.0, 0.0, 0.0, // 1
            0.0, 1.0, 0.0, // 2
            5.0, 5.0, 5.0, // 3
            5.00005, 5.0, 5.0, // 4: merges into 3
            6.0, 5.0, 5.0, // 5
            1.00008, 0.0, 0.0, // 6: merges into 1
            1.0, 1.0, 1.0, // 7
            0.0003, 0.0, 0.0, // 8: too far from 0
        ];
        let mesh = Mesh {
            positions,
            indices: vec![0, 1, 2, 3, 4, 5, 6, 7, 8],
        };
        let out = sanitize(&mesh, 0.0001);
        assert!(out.changed);
        assert_eq!(
            out.message,
            "Merged 2 nodes, removed 1 degenerate triangle."
        );
        assert_eq!(out.origin, Origin::Map(vec![0, 2]));
        assert_eq!(out.mesh.triangle_count(), 2);
        // The first position wins: vertex 6's corner now sits exactly on vertex 1.
        let t = &out.mesh.indices;
        assert_eq!(t[0..3], [0, 1, 2]);
        assert_eq!(out.mesh.vertex(t[3]), [1.0, 0.0, 0.0]);
        assert_eq!(out.mesh.vertex(t[5]), [0.0003, 0.0, 0.0]);
        // Vertices only the collapsed triangle used are gone.
        assert_eq!(out.mesh.vertex_count(), 5);
        assert!(!sanitize(&out.mesh, 0.0001).changed, "idempotent");
    }

    #[test]
    fn the_tolerance_decides_what_merges() {
        let positions = vec![
            0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, //
            0.01, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let mesh = Mesh {
            positions,
            indices: vec![0, 1, 2, 3, 4, 5],
        };
        assert!(!sanitize(&mesh, 0.001).changed);
        let out = sanitize(&mesh, 0.02);
        assert_eq!(out.message, "Merged 1 node.");
        assert_eq!(out.mesh.vertex_count(), 5);
        assert_eq!(out.mesh.indices, vec![0, 1, 2, 0, 3, 4]);
    }

    #[test]
    fn a_zero_tolerance_merges_equal_positions_only() {
        let cube = make_box([0.0; 3], [20.0; 3]);
        let mut mesh = soup(&cube);
        // A corner a hair away from its twin stays apart at tolerance 0.
        mesh.positions[0] = f32::from_bits(0.0f32.to_bits() + 1);
        let out = sanitize(&mesh, 0.0);
        assert_eq!(out.mesh.vertex_count(), 9);
        // -0.0 and +0.0 are the same position.
        let mut negative = soup(&cube);
        for v in negative
            .positions
            .iter_mut()
            .filter(|v| **v == 0.0)
            .step_by(2)
        {
            *v = -0.0;
        }
        assert_eq!(sanitize(&negative, 0.0).mesh.vertex_count(), 8);
    }

    #[test]
    fn a_zero_tolerance_stays_fast_on_a_big_soup() {
        // Every vertex lands in one grid cell if weld_within gets a zero tolerance; 300,000
        // corners would then take minutes.
        let n = 100_000u32;
        let mut positions = Vec::with_capacity(n as usize * 9);
        for i in 0..n {
            let x = i as f32;
            positions.extend_from_slice(&[x, 0.0, 0.0, x, 1.0, 0.0, x + 1.0, 0.0, 0.0]);
        }
        let mesh = Mesh {
            positions,
            indices: (0..n * 3).collect(),
        };
        let started = std::time::Instant::now();
        let out = sanitize(&mesh, 0.0);
        assert!(started.elapsed().as_secs_f64() < 20.0);
        // Each triangle's third corner is the next one's first.
        assert_eq!(
            out.mesh.vertex_count(),
            mesh.vertex_count() - (n as usize - 1)
        );
    }

    #[test]
    fn unused_vertices_take_no_part_and_are_dropped() {
        // Vertex 0 is unused and lies within the tolerance of vertex 1: vertex 1 must keep its
        // own position.
        let positions = vec![
            0.00005, 0.0, 0.0, // 0: unused
            0.0, 0.0, 0.0, // 1
            1.0, 0.0, 0.0, // 2
            0.0, 1.0, 0.0, // 3
        ];
        let mesh = Mesh {
            positions,
            indices: vec![1, 2, 3],
        };
        let out = sanitize(&mesh, 0.0001);
        assert!(out.changed);
        assert_eq!(out.message, "Dropped 1 unused node.");
        assert_eq!(out.origin, Origin::Same);
        assert_eq!(out.mesh.positions[..3], [0.0, 0.0, 0.0]);
        assert_eq!(out.mesh.indices, vec![0, 1, 2]);
    }

    #[test]
    fn existing_degenerate_triangles_go_and_the_selection_follows() {
        let mut cube = make_box([0.0; 3], [20.0; 3]);
        let copy = cube.indices[3..6].to_vec();
        cube.indices.extend_from_slice(&[0, 0, 1]);
        cube.indices.extend_from_slice(&copy);
        let out = sanitize(&cube, 0.0001);
        assert_eq!(out.message, "Removed 1 degenerate triangle.");
        let mut want: Vec<u32> = (0..12).collect();
        want.push(13);
        assert_eq!(out.origin, Origin::Map(want));
        // Selected: triangle 1, the degenerate 12 and the copy 13; 12 is gone, 13 is now 12.
        assert_eq!(
            remap_selection(&[1, 12, 13], &out.origin, out.mesh.triangle_count()),
            vec![1, 12]
        );
        let Origin::Map(map) = &out.origin else {
            unreachable!()
        };
        assert!(!map.contains(&NEW));
    }

    #[test]
    fn everything_merging_into_one_point_leaves_nothing() {
        let tiny = make_box([0.0; 3], [0.00001; 3]);
        let out = sanitize(&tiny, 0.0001);
        assert_eq!(out.mesh.triangle_count(), 0);
        assert_eq!(out.mesh.vertex_count(), 0);
        assert_eq!(out.origin, Origin::Map(Vec::new()));
        assert_eq!(
            out.message,
            "Merged 7 nodes, removed 12 degenerate triangles."
        );
        assert!(!sanitize(&out.mesh, 0.0001).changed);
    }

    #[test]
    fn a_cancelled_run_stops() {
        let cancel = AtomicBool::new(true);
        let mut control = Control::new(&cancel, |_| {});
        let cube = make_box([0.0; 3], [20.0; 3]);
        assert_eq!(
            run(&soup(&cube), &[], &TOLERANCE, &mut control),
            Err(RepairError::Cancelled)
        );
    }

    #[test]
    fn progress_reaches_the_end_and_never_goes_back() {
        let never = AtomicBool::new(false);
        let mut seen = Vec::new();
        {
            let mut control = Control::new(&never, |f| seen.push(f));
            let cube = make_box([0.0; 3], [20.0; 3]);
            run(&soup(&cube), &[], &TOLERANCE, &mut control).unwrap();
        }
        assert!(seen.windows(2).all(|w| w[1] >= w[0]));
        assert_eq!(seen.last(), Some(&1.0));
    }

    /// `cargo test --release --lib repair::sanitize -- --ignored --nocapture`: a 1M-triangle soup.
    #[test]
    #[ignore]
    fn timing_on_a_million_triangles() {
        let n = 1_000_000u32;
        let side = (n as f64 / 2.0).sqrt().ceil() as u32;
        let mut positions = Vec::with_capacity(n as usize * 9);
        'grid: for i in 0..side {
            for j in 0..side {
                let (x, y) = (i as f32 * 0.1, j as f32 * 0.1);
                for t in [
                    [[x, y], [x + 0.1, y], [x + 0.1, y + 0.1]],
                    [[x, y], [x + 0.1, y + 0.1], [x, y + 0.1]],
                ] {
                    for [px, py] in t {
                        positions.extend_from_slice(&[px, py, 0.00002 * (px + py)]);
                    }
                    if positions.len() >= n as usize * 9 {
                        break 'grid;
                    }
                }
            }
        }
        let mesh = Mesh {
            indices: (0..positions.len() as u32 / 3).collect(),
            positions,
        };
        for tolerance in [0.0001, 0.0] {
            let started = std::time::Instant::now();
            let out = sanitize(&mesh, tolerance);
            println!(
                "sanitize {} triangles, tolerance {tolerance}: {:.0} ms, {} -> {} vertices",
                mesh.triangle_count(),
                started.elapsed().as_secs_f64() * 1000.0,
                mesh.vertex_count(),
                out.mesh.vertex_count()
            );
        }
    }
}
