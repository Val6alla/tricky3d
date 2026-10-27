//! Home › Repair › Bridge a gap: joins two open edges that face each other across a gap with two
//! triangles. It moved here from the window (src/repair/edits.ts in PR #1) and picks the same two
//! edges; where the window compared every pair of open edges, the nearest pairs are found here
//! with a sweep, so a part with thousands of open edges does not stall.
//!
//! With no gap to bridge, it stitches open edges instead and, with a selection, closes the holes
//! that border it, as the window's fallback did, all as one step.

use rayon::prelude::*;

use super::defaults::STITCH;
use super::exact::{P3, cross, dot, sub, vertex};
use super::open_edges::{OpenSide, appended, facing, open_sides, triangle_normal};
use super::topology::Edges;
use super::{Control, Outcome, RepairResult, compose_origin, holes, remap_selection, stitch};
use crate::mesh::Mesh;

/// The nearest pairs whose bridge is checked against the mesh, nearest first; the first whose
/// middle is not on the mesh is bridged.
const CANDIDATES: usize = 8;
/// Sides between cancel checks in the sweep.
const CHECK_EVERY: usize = 1024;

/// Bridges the nearest two open sides that run opposite ways, share no corner and are not the two
/// sides of a strip already joined by a triangle, unless the middle of the bridge lies on the mesh
/// (they are two sides of one surface, not of a gap). Only the CANDIDATES nearest pairs are
/// tried. With a selection, only open sides of selected triangles count. With no such pair:
/// Stitch triangles (default values), then with a selection Close defined holes.
pub fn run(mesh: &Mesh, selection: &[u32], control: &mut Control) -> RepairResult<Outcome> {
    control.check()?;
    let edges = Edges::new(mesh);
    let sides = open_sides(mesh, &edges, selection);
    drop(edges);
    control.progress(0.1);
    let found = gap(mesh, &sides, &mut control.sub(0.1, 0.5))?;
    if let Some((e, f)) = found {
        let hint = triangle_normal(mesh, e.triangle);
        let at = |v: u32| vertex(mesh, v);
        let mut out = mesh.clone();
        // Each new triangle runs against the open side it covers, so that edge gets its second.
        out.indices
            .extend_from_slice(&facing(&at, [e.b, e.a, f.b], hint));
        out.indices
            .extend_from_slice(&facing(&at, [e.b, f.b, f.a], hint));
        control.progress(1.0);
        return Ok(appended(
            mesh,
            out,
            "Bridged a gap with 2 triangles.".into(),
        ));
    }
    let stitched = stitch::run(mesh, selection, &STITCH, &mut control.sub(0.5, 0.8))?;
    let mut message = format!("No gap to bridge. {}", stitched.message);
    let mut out = Outcome {
        message: String::new(),
        ..stitched
    };
    if !selection.is_empty() {
        let after = if out.changed {
            remap_selection(selection, &out.origin, out.mesh.triangle_count())
        } else {
            selection.to_vec()
        };
        let closed = holes::close_defined(&out.mesh, &after, &mut control.sub(0.8, 1.0))?;
        message = format!("{message} {}", closed.message);
        let offset = out.warnings.len();
        out.warnings.extend(closed.warnings);
        out.concerns
            .extend(closed.concerns.into_iter().map(|(w, c)| (w + offset, c)));
        if closed.changed {
            out.origin = compose_origin(&out.origin, &closed.origin);
            out.mesh = closed.mesh;
            out.changed = true;
        }
    }
    out.message = message;
    Ok(out)
}

/// The two open sides to bridge, if any (see `run`).
fn gap(
    mesh: &Mesh,
    sides: &[OpenSide],
    control: &mut Control,
) -> RepairResult<Option<(OpenSide, OpenSide)>> {
    let mids: Vec<P3> = sides
        .iter()
        .map(|s| {
            let (a, b) = (mesh.vertex(s.a), mesh.vertex(s.b));
            [0, 1, 2].map(|k| (a[k] as f64 + b[k] as f64) / 2.0)
        })
        .collect();
    let runs: Vec<P3> = sides
        .iter()
        .map(|s| sub(vertex(mesh, s.b), vertex(mesh, s.a)))
        .collect();
    let star = Star::new(mesh);
    let nearest = nearest_pairs(sides, &mids, &runs, &star, control)?;
    control.progress(0.8);
    for (_, i, j) in nearest {
        control.check()?;
        let (e, f) = (sides[i], sides[j]);
        let corners = [e.a, e.b, f.a, f.b].map(|v| mesh.vertex(v));
        let middle: P3 = [0, 1, 2].map(|k| {
            (corners[0][k] as f64
                + corners[1][k] as f64
                + corners[2][k] as f64
                + corners[3][k] as f64)
                / 4.0
        });
        let on_mesh = (0..mesh.triangle_count() as u32)
            .into_par_iter()
            .any(|t| on_triangle(mesh, t, middle));
        if !on_mesh {
            return Ok(Some((e, f)));
        }
    }
    Ok(None)
}

/// The CANDIDATES best pairs (i < j) by (squared distance between the sides' midpoints, i, j),
/// among pairs that share no corner, run opposite ways and are not already joined by a triangle,
/// best first: the order a stable sort of every pair by distance gives.
///
/// Sweep along the axis the midpoints spread most along: sorted by that coordinate, each side is
/// paired with the ones after it until their distance along the axis alone exceeds the worst
/// pair kept. A squared distance is never below the square of one of its terms (each term is
/// rounded up or down as a sum of non-negative numbers is), so no better pair is skipped.
fn nearest_pairs(
    sides: &[OpenSide],
    mids: &[P3],
    runs: &[P3],
    star: &Star,
    control: &mut Control,
) -> RepairResult<Vec<(f64, usize, usize)>> {
    let mut best: Vec<(f64, usize, usize)> = Vec::with_capacity(CANDIDATES + 1);
    if sides.len() < 2 {
        return Ok(best);
    }
    let spread = |k: usize| {
        let (lo, hi) = mids
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), m| {
                (lo.min(m[k]), hi.max(m[k]))
            });
        hi - lo
    };
    let axis = (0..3)
        .max_by(|&a, &b| spread(a).total_cmp(&spread(b)))
        .unwrap_or(0);
    let mut order: Vec<usize> = (0..sides.len()).collect();
    order.sort_by(|&i, &j| mids[i][axis].total_cmp(&mids[j][axis]).then(i.cmp(&j)));
    let worse = |a: &(f64, usize, usize), b: &(f64, usize, usize)| {
        a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2))
    };
    for (x, &first) in order.iter().enumerate() {
        if x % CHECK_EVERY == 0 {
            control.check()?;
            control.progress(x as f64 / order.len() as f64);
        }
        for &second in &order[x + 1..] {
            let along = mids[second][axis] - mids[first][axis];
            if best.len() == CANDIDATES && along * along > best[CANDIDATES - 1].0 {
                break;
            }
            let (i, j) = (first.min(second), first.max(second));
            let (e, f) = (sides[i], sides[j]);
            if e.a == f.a || e.a == f.b || e.b == f.a || e.b == f.b {
                continue;
            }
            if dot(runs[i], runs[j]) >= 0.0 {
                continue;
            }
            // The two sides of a strip already have these triangles; bridging them would stack a
            // second copy.
            if star.has([e.b, e.a, f.b]) || star.has([e.b, f.b, f.a]) {
                continue;
            }
            let d = [0, 1, 2].map(|k| mids[i][k] - mids[j][k]);
            let pair = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2], i, j);
            if best.len() < CANDIDATES || worse(&pair, &best[CANDIDATES - 1]).is_lt() {
                let at = best.partition_point(|b| worse(b, &pair).is_lt());
                best.insert(at, pair);
                best.truncate(CANDIDATES);
            }
        }
    }
    Ok(best)
}

/// The triangles round each vertex, to ask whether a triangle over three vertices exists.
struct Star {
    start: Vec<u32>,
    triangles: Vec<u32>,
    corners: Vec<[u32; 3]>,
}

impl Star {
    fn new(mesh: &Mesh) -> Star {
        let corners: Vec<[u32; 3]> = mesh
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&t| sorted(t))
            .collect();
        let count = mesh.vertex_count();
        let mut start = vec![0u32; count + 1];
        for &v in &mesh.indices {
            start[v as usize + 1] += 1;
        }
        for v in 0..count {
            start[v + 1] += start[v];
        }
        let mut fill = start[..count].to_vec();
        let mut triangles = vec![0u32; mesh.indices.len()];
        for (t, c) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
            for &v in c {
                triangles[fill[v as usize] as usize] = t as u32;
                fill[v as usize] += 1;
            }
        }
        Star {
            start,
            triangles,
            corners,
        }
    }

    /// Whether some triangle has exactly these three corners, in any order.
    fn has(&self, corners: [u32; 3]) -> bool {
        let key = sorted(corners);
        let v = corners[0] as usize;
        self.triangles[self.start[v] as usize..self.start[v + 1] as usize]
            .iter()
            .any(|&t| self.corners[t as usize] == key)
    }
}

fn sorted(mut t: [u32; 3]) -> [u32; 3] {
    t.sort_unstable();
    t
}

/// Whether `p` lies on mesh triangle `t`, within the window's tolerances: inside its box grown by
/// 1e-4 mm, at most 1e-4 mm from its plane, and inside its edges by barycentric coordinates
/// down to −1e-6. A triangle without area holds no point.
fn on_triangle(mesh: &Mesh, t: u32, p: P3) -> bool {
    let i = t as usize * 3;
    let [a, b, c] = [0, 1, 2].map(|k| vertex(mesh, mesh.indices[i + k]));
    for k in 0..3 {
        let lo = a[k].min(b[k]).min(c[k]);
        let hi = a[k].max(b[k]).max(c[k]);
        if p[k] < lo - 1e-4 || p[k] > hi + 1e-4 {
            return false;
        }
    }
    let ab = sub(b, a);
    let ac = sub(c, a);
    let n = cross(ab, ac);
    let nn = dot(n, n);
    if nn == 0.0 {
        return false;
    }
    let ap = sub(p, a);
    if (dot(ap, n) * dot(ap, n)) / nn > 1e-8 {
        return false;
    }
    let d00 = dot(ab, ab);
    let d01 = dot(ab, ac);
    let d11 = dot(ac, ac);
    let d20 = dot(ap, ab);
    let d21 = dot(ap, ac);
    let denom = d00 * d11 - d01 * d01;
    if denom == 0.0 {
        return false;
    }
    let v = (d11 * d20 - d01 * d21) / denom;
    let w = (d00 * d21 - d01 * d20) / denom;
    let u = 1.0 - v - w;
    u >= -1e-6 && v >= -1e-6 && w >= -1e-6
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::mesh::{make_box, stats};
    use crate::repair::exact::testing::Rng;
    use crate::repair::topology::is_consistent;
    use crate::repair::{NEW, Origin, RepairError, silent};

    fn tri(positions: &[f32], indices: &[u32]) -> Mesh {
        Mesh {
            positions: positions.to_vec(),
            indices: indices.to_vec(),
        }
    }

    /// Two triangles 1 mm apart in z, one above the other.
    fn gap_pair() -> Mesh {
        tri(
            &[
                0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0,
                1.0, 1.0,
            ],
            &[0, 1, 2, 3, 4, 5],
        )
    }

    /// A strip of `n` unit squares along x, two triangles each.
    fn strip(n: u32) -> Mesh {
        let mut positions = Vec::new();
        for i in 0..=n {
            positions.extend_from_slice(&[i as f32, 0.0, 0.0, i as f32, 1.0, 0.0]);
        }
        let mut indices = Vec::new();
        for i in 0..n {
            let (a, b, c, d) = (2 * i, 2 * i + 2, 2 * i + 3, 2 * i + 1);
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
        Mesh { positions, indices }
    }

    #[test]
    fn two_opposing_edges_across_a_gap_get_two_triangles() {
        let m = gap_pair();
        let out = silent(|c| run(&m, &[], c)).unwrap();
        assert!(out.changed);
        assert_eq!(out.message, "Bridged a gap with 2 triangles.");
        assert_eq!(out.mesh.positions, m.positions);
        assert_eq!(out.origin, Origin::Map(vec![0, 1, NEW, NEW]));
        assert_eq!(out.select, Some(Vec::new()));
        // The two bridged edges and the bridge's diagonal have two sides each, running opposite
        // ways, and no edge has three.
        let edges = Edges::new(&out.mesh);
        let joined: Vec<u32> = (0..edges.len() as u32)
            .filter(|&e| edges.degree(e) == 2)
            .collect();
        assert_eq!(joined.len(), 3);
        assert!(joined.iter().all(|&e| is_consistent(&out.mesh, &edges, e)));
        assert_eq!(stats(&out.mesh).non_manifold_edges, 0);
    }

    #[test]
    fn an_open_strip_is_not_bridged_across_itself() {
        // Opposite sides of one strip: either a triangle joins them already, or the bridge's
        // middle lies on the strip. PR #1 left the strip open for Stitch, and so does this.
        for n in [1, 3] {
            let m = strip(n);
            let edges = Edges::new(&m);
            let sides = open_sides(&m, &edges, &[]);
            assert_eq!(silent(|c| gap(&m, &sides, c)).unwrap(), None);
            let out = silent(|c| run(&m, &[], c)).unwrap();
            assert!(!out.changed);
            assert!(
                out.message.starts_with("No gap to bridge. "),
                "{}",
                out.message
            );
        }
    }

    /// A 20 mm cube whose top face has vertices of its own, `lift` mm above the others: a crack
    /// all round the top.
    fn cracked_cube(lift: f32) -> Mesh {
        let mut m = make_box([0.0; 3], [20.0; 3]);
        let base = m.vertex_count() as u32;
        let mut lifted: Vec<u32> = m.indices[6..12].to_vec();
        lifted.sort_unstable();
        lifted.dedup();
        for &v in &lifted {
            let p = m.vertex(v);
            m.positions.extend_from_slice(&[p[0], p[1], p[2] + lift]);
        }
        for i in 6..12 {
            let k = lifted.binary_search(&m.indices[i]).unwrap();
            m.indices[i] = base + k as u32;
        }
        m
    }

    #[test]
    fn a_crack_is_bridged_across() {
        // 0.005 mm wide: the nearest opposing edges, and the bridge's middle is off the mesh.
        // The two edges it joins get their second side, and the bridge's own two ends are open
        // edges in their place.
        let m = cracked_cube(0.005);
        let out = silent(|c| run(&m, &[], c)).unwrap();
        assert_eq!(out.message, "Bridged a gap with 2 triangles.");
        assert_eq!(stats(&out.mesh).open_edges, 8);
        assert_eq!(stats(&out.mesh).non_manifold_edges, 0);
    }

    #[test]
    fn with_no_gap_it_stitches() {
        // No width: every bridge would lie on the mesh, so Stitch joins the crack instead.
        let m = cracked_cube(0.0);
        assert_eq!(stats(&m).open_edges, 8);
        let out = silent(|c| run(&m, &[], c)).unwrap();
        assert!(out.changed, "{}", out.message);
        assert!(
            out.message.starts_with("No gap to bridge. "),
            "{}",
            out.message
        );
        assert_eq!(out.select, None, "the selection follows its triangles");
        assert_eq!(stats(&out.mesh).open_edges, 0);
        assert_eq!(stats(&out.mesh).volume, 8000.0);
    }

    #[test]
    fn with_no_gap_and_a_selection_it_closes_the_holes_beside_it() {
        // As the window's fallback did: with nothing to bridge or stitch, Close defined holes
        // runs on the selection, which here closes a strip's own outline (a flat lid on its back)
        // and, for one selected triangle of the gap pair, that triangle's outline.
        let m = strip(3);
        let alone = silent(|c| run(&m, &[], c)).unwrap();
        assert!(!alone.changed);
        let out = silent(|c| run(&m, &[0], c)).unwrap();
        assert!(out.changed, "{}", out.message);
        assert!(
            out.message.starts_with("No gap to bridge. "),
            "{}",
            out.message
        );
        assert!(
            out.message.ends_with("Closed 1 hole, added 6 triangles."),
            "{}",
            out.message
        );
        assert_eq!(stats(&out.mesh).open_edges, 0);
        let one = silent(|c| run(&gap_pair(), &[0], c)).unwrap();
        assert_eq!(one.mesh.triangle_count(), 3, "{}", one.message);
    }

    /// The pairs as PR #1 ranked them: every valid pair sorted by distance, stably (ties keep
    /// (i, j) order), the first CANDIDATES.
    fn ranked_by_brute_force(
        sides: &[OpenSide],
        mids: &[P3],
        runs: &[P3],
        star: &Star,
    ) -> Vec<(f64, usize, usize)> {
        let mut all = Vec::new();
        for i in 0..sides.len() {
            for j in i + 1..sides.len() {
                let (e, f) = (sides[i], sides[j]);
                if e.a == f.a || e.a == f.b || e.b == f.a || e.b == f.b {
                    continue;
                }
                if dot(runs[i], runs[j]) >= 0.0 {
                    continue;
                }
                if star.has([e.b, e.a, f.b]) || star.has([e.b, f.b, f.a]) {
                    continue;
                }
                let d = [0, 1, 2].map(|k| mids[i][k] - mids[j][k]);
                all.push((d[0] * d[0] + d[1] * d[1] + d[2] * d[2], i, j));
            }
        }
        all.sort_by(|a, b| a.0.total_cmp(&b.0));
        all.truncate(CANDIDATES);
        all
    }

    #[test]
    fn the_sweep_finds_the_pairs_a_full_ranking_finds() {
        let mut rng = Rng(5);
        for round in 0..200 {
            // A soup of small triangles on a coarse grid, so that equal distances happen often.
            let count = 2 + rng.below(60) as usize;
            let grid = if round % 2 == 0 { 4 } else { 50 };
            let mut m = Mesh::default();
            for t in 0..count {
                for _ in 0..9 {
                    m.positions.push(rng.int(0, grid) as f32 * 0.5);
                }
                let base = 3 * t as u32;
                m.indices.extend_from_slice(&[base, base + 1, base + 2]);
            }
            let edges = Edges::new(&m);
            let sides = open_sides(&m, &edges, &[]);
            let mids: Vec<P3> = sides
                .iter()
                .map(|s| {
                    let (a, b) = (m.vertex(s.a), m.vertex(s.b));
                    [0, 1, 2].map(|k| (a[k] as f64 + b[k] as f64) / 2.0)
                })
                .collect();
            let runs: Vec<P3> = sides
                .iter()
                .map(|s| sub(vertex(&m, s.b), vertex(&m, s.a)))
                .collect();
            let star = Star::new(&m);
            let swept = silent(|c| nearest_pairs(&sides, &mids, &runs, &star, c)).unwrap();
            let all = ranked_by_brute_force(&sides, &mids, &runs, &star);
            assert_eq!(swept, all, "round {round}");
        }
    }

    #[test]
    fn a_cancelled_bridge_stops() {
        let cancel = AtomicBool::new(true);
        let mut control = Control::new(&cancel, |_| {});
        assert_eq!(
            run(&gap_pair(), &[], &mut control),
            Err(RepairError::Cancelled)
        );
    }
}
