//! Stitch triangles: join open border edges that lie within the tolerance of each other and run
//! nearly parallel, closing small gaps and cracks. With a selection, only border edges of selected
//! triangles are stitched.
//!
//! Each pass first joins pairs of border sides. A border side a→b pairs with a border side c→d when
//! c lies within the tolerance of b and d within the tolerance of a: it runs the other way, so the
//! joined edge is consistently oriented. With Ignore orientation, sides running the same way (c
//! near a, d near b) pair too. Pairs are taken greedily, closest first (by the larger of the two
//! end distances), and their ends merged; the lower vertex index keeps its position. Then come the
//! T-junctions: a border vertex within the tolerance of the inside of another border side, along
//! which the vertex's own border runs, is put into that side by splitting the side's triangle
//! there, so the two lips get the same vertices and their sides join. Passes repeat until nothing
//! more stitches.
//!
//! Every merge and split is checked against the triangles around it and refused when it would put
//! three triangles on an edge, make two triangles coincide, turn a triangle over, collapse one of
//! the two joined sides' own triangles, or, without Ignore orientation, join two triangles facing
//! opposite ways. Triangles that a merge leaves with two equal corners (corners within the
//! tolerance of each other) are removed. After the first scan for border edges only the triangles
//! around border vertices are looked at, so a mesh with thousands of border edges stitches in
//! milliseconds, and a closed mesh comes back unchanged after that one scan.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::atomic::Ordering;

use rayon::prelude::*;

use super::bvh::{Aabb, Bvh};
use super::exact::{P3, add, cross, dot, normal, scale, sub, vertex};
use super::topology::{Edges, NONE};
use super::{Control, Origin, Outcome, RepairResult, StitchParams};
use crate::mesh::{Mesh, fmt_count};
use crate::ops::compact;

/// Most passes. Real cracks close in a handful; each pass after the first only looks at the
/// triangles around the remaining border.
const MAX_PASSES: usize = 100;

/// Loop iterations between cancel checks.
const CHECK_EVERY: usize = 4096;

/// Joins every pair of border edges that fit within `params.tolerance` mm and lines up the
/// T-junctions where two lips have different vertices (see the module doc). The result keeps the
/// triangle order; pieces of split triangles come last and map to the triangle they came from.
/// Nothing changes unless the border shrinks.
pub fn run(
    mesh: &Mesh,
    selection: &[u32],
    params: &StitchParams,
    control: &mut Control,
) -> RepairResult<Outcome> {
    control.check()?;
    let edges = Edges::new(mesh);
    let mut on_border = vec![false; mesh.vertex_count()];
    let mut sides = Vec::new();
    for e in 0..edges.len() as u32 {
        if edges.degree(e) == 1 {
            sides.push(edges.sides(e)[0]);
            for v in edges.ends(e) {
                on_border[v as usize] = true;
            }
        }
    }
    drop(edges);
    if sides.is_empty() {
        return Ok(Outcome::unchanged(mesh, "No border edges to stitch."));
    }
    let border_edges = sides.len();
    sides.sort_unstable();
    control.check()?;
    control.progress(0.2);
    let mut work = Stitcher::new(mesh, &on_border, selection, params);
    drop(on_border);
    // A second run must see exactly the border a pass after a change sees.
    debug_assert_eq!(work.border_sides(), sides);
    if work.sides_taking_part(&sides).is_empty() {
        return Ok(Outcome::unchanged(
            mesh,
            "The selected triangles have no border edges to stitch.",
        ));
    }
    // `sides` always holds the current border sides; it is counted again only after a change.
    for pass in 0..MAX_PASSES {
        control.check()?;
        let joined = work.join_pairs(&sides, control)?;
        if joined > 0 {
            sides = work.border_sides();
        }
        let split = work.split_t_junctions(&sides, control)?;
        if split > 0 {
            sides = work.border_sides();
        }
        control.progress(0.2 + 0.7 * (1.0 - 1.0 / (pass + 2) as f64));
        if joined + split == 0 {
            break;
        }
    }
    let left = sides.len();
    if left >= border_edges {
        return Ok(Outcome::unchanged(
            mesh,
            "No border edges close enough to stitch.",
        ));
    }
    let (out, origin) = work.finish();
    control.progress(1.0);
    // Two border edges joined into one edge count as one stitched edge.
    let stitched = (border_edges - left).div_ceil(2);
    let unit = if stitched == 1 { "edge" } else { "edges" };
    Ok(Outcome {
        mesh: out,
        changed: true,
        origin,
        message: format!("Stitched {} {unit}.", fmt_count(stitched as u64)),
        warnings: Vec::new(),
        highlight: None,
        select: None,
        concerns: Vec::new(),
    })
}

/// A pair of border sides that may be joined: their positions in the pass's side list, the larger
/// of their two end distances (squared), and whether they run the same way.
struct Pair {
    cost: f64,
    same: bool,
    i: u32,
    j: u32,
}

/// A border vertex that may go into a border side: the vertex, the side's position in the pass's
/// side list, where along the side it lies (0 at the start, 1 at the end), and its distance
/// (squared) from the side.
struct Junction {
    distance: f64,
    vertex: u32,
    side: u32,
    t: f64,
}

/// Buffers a join check fills, kept between checks so a pass of thousands allocates nothing.
#[derive(Default)]
struct Scratch {
    triangles: Vec<u32>,
    collapsing: Vec<u32>,
    sides: Vec<(u64, u64, bool)>,
    faces: Vec<([u32; 3], [u32; 3])>,
}

/// The stitch in progress. Vertices never move: a merged vertex takes its group's position, which
/// is that of the group's root, its lowest vertex. Triangles keep their input numbers, and the
/// second pieces of split triangles are appended.
struct Stitcher<'a> {
    mesh: &'a Mesh,
    tolerance: f64,
    ignore_orientation: bool,
    /// Grid cell width for the pair search: four tolerances, or more when the tolerance is so
    /// small against the coordinates that cell numbers would overflow.
    cell: f64,
    /// Input triangles, then the pieces splits add. Corners are input vertex ids; `root` gives the
    /// vertex each one is now.
    triangles: Vec<[u32; 3]>,
    /// The input triangle each one came from.
    origin: Vec<u32>,
    /// Triangles a merge collapsed (two corners joined); they are dropped at the end.
    collapsed: Vec<bool>,
    /// Per vertex: the root of its group, itself for every vertex no merge touched.
    root: Vec<u32>,
    /// Per vertex: its number among the border vertices, NONE for the others. Only border vertices
    /// merge or go into sides, so the bookkeeping below is per border vertex.
    local: Vec<u32>,
    /// The border vertices in order (local number → vertex).
    border_vertices: Vec<u32>,
    /// Per border vertex: the next member of its group, NONE at the end (a root's list holds its
    /// whole group).
    next_member: Vec<u32>,
    /// Per root border vertex: the triangles around its group (collapsed ones may linger).
    around: Vec<Vec<u32>>,
    /// The triangles with two or more border-vertex corners: the only ones with sides that are, or
    /// after merges and splits can become, border sides.
    region: Vec<u32>,
    /// Per input triangle: whether its border sides take part (None: all do).
    taking_part: Option<Vec<bool>>,
    scratch: Scratch,
}

impl<'a> Stitcher<'a> {
    fn new(
        mesh: &'a Mesh,
        on_border: &[bool],
        selection: &[u32],
        params: &StitchParams,
    ) -> Stitcher<'a> {
        let count = mesh.vertex_count();
        let mut local = vec![NONE; count];
        let mut border_vertices = Vec::new();
        for (v, _) in on_border.iter().enumerate().filter(|&(_, &b)| b) {
            local[v] = border_vertices.len() as u32;
            border_vertices.push(v as u32);
        }
        let triangles: Vec<[u32; 3]> = mesh.indices.as_chunks::<3>().0.to_vec();
        let mut around = vec![Vec::new(); border_vertices.len()];
        let mut region = Vec::new();
        for (t, c) in triangles.iter().enumerate() {
            let mut corners = 0;
            for (k, &v) in c.iter().enumerate() {
                if local[v as usize] != NONE && !c[..k].contains(&v) {
                    around[local[v as usize] as usize].push(t as u32);
                    corners += 1;
                }
            }
            if corners >= 2 {
                region.push(t as u32);
            }
        }
        let taking_part = (!selection.is_empty()).then(|| {
            let mut mask = vec![false; mesh.triangle_count()];
            for &t in selection {
                if let Some(slot) = mask.get_mut(t as usize) {
                    *slot = true;
                }
            }
            mask
        });
        // Any non-negative tolerance works (NaN counts as 0). Cells four tolerances wide put the
        // neighbourhood of a point into 1 to 8 cells, as in sanitize's weld; cell numbers stay
        // below 2^41.
        let tolerance = params.tolerance.max(0.0);
        let largest = border_vertices
            .iter()
            .flat_map(|&v| vertex(mesh, v))
            .fold(0.0f64, |m, x| m.max(x.abs()));
        let cell = (4.0 * tolerance)
            .max(largest * 2f64.powi(-40))
            .max(f64::MIN_POSITIVE);
        Stitcher {
            mesh,
            tolerance,
            ignore_orientation: params.ignore_orientation,
            cell,
            origin: (0..triangles.len() as u32).collect(),
            collapsed: vec![false; triangles.len()],
            triangles,
            root: (0..count as u32).collect(),
            local,
            next_member: vec![NONE; border_vertices.len()],
            border_vertices,
            around,
            region,
            taking_part,
            scratch: Scratch::default(),
        }
    }

    /// Where vertex `v` is now.
    #[inline]
    fn at(&self, v: u32) -> P3 {
        vertex(self.mesh, self.root[v as usize])
    }

    /// Triangle `t`'s corners as they are now.
    #[inline]
    fn corners(&self, t: u32) -> [u32; 3] {
        self.triangles[t as usize].map(|v| self.root[v as usize])
    }

    /// The vertex a side starts at and the one it ends at, as they are now.
    #[inline]
    fn ends(&self, side: u32) -> (u32, u32) {
        let c = self.corners(side / 3);
        let k = (side % 3) as usize;
        (c[k], c[(k + 1) % 3])
    }

    fn takes_part(&self, t: u32) -> bool {
        self.taking_part
            .as_ref()
            .is_none_or(|selected| selected[self.origin[t as usize] as usize])
    }

    /// The current border sides, ascending. Every border side joins two border vertices and lies
    /// in a region triangle, and so do all other sides on its edge, so counting sides over the
    /// region counts them fully.
    fn border_sides(&self) -> Vec<u32> {
        let mut keys: Vec<u128> = Vec::with_capacity(self.region.len() * 2);
        for &t in &self.region {
            if self.collapsed[t as usize] {
                continue;
            }
            let c = self.corners(t);
            for k in 0..3 {
                let (p, q) = (c[k], c[(k + 1) % 3]);
                if p != q && self.local[p as usize] != NONE && self.local[q as usize] != NONE {
                    keys.push(((edge_key(p, q) as u128) << 32) | (t * 3 + k as u32) as u128);
                }
            }
        }
        // Every key is distinct (it holds its side), so the order does not depend on the threads.
        keys.par_sort_unstable();
        let mut sides: Vec<u32> = keys
            .chunk_by(|a, b| a >> 32 == b >> 32)
            .filter(|group| group.len() == 1)
            .map(|group| group[0] as u32)
            .collect();
        sides.par_sort_unstable();
        sides
    }

    /// The border sides among `sides` whose triangles take part.
    fn sides_taking_part(&self, sides: &[u32]) -> Vec<u32> {
        sides
            .iter()
            .copied()
            .filter(|&side| self.takes_part(side / 3))
            .collect()
    }

    /// One round of pair joins (module doc) among the current border sides; the number joined.
    fn join_pairs(&mut self, border: &[u32], control: &mut Control) -> RepairResult<usize> {
        let sides = self.sides_taking_part(border);
        if sides.len() < 2 {
            return Ok(0);
        }
        let tolerance = self.tolerance;
        let limit = tolerance * tolerance;
        let at: Vec<(P3, P3)> = sides
            .iter()
            .map(|&s| {
                let (a, b) = self.ends(s);
                (self.at(a), self.at(b))
            })
            .collect();
        let starts: Vec<P3> = at.iter().map(|&(a, _)| a).collect();
        let grid = Grid::new(&starts, self.cell);
        let cancel = control.cancel_flag();
        let ignore_orientation = self.ignore_orientation;
        // Each side looks for its partners on its own, so the search runs on every core; the sort
        // below fixes the order whatever the threads did.
        let mut pairs: Vec<Pair> = (0..at.len())
            .into_par_iter()
            .flat_map_iter(|i| {
                let mut found = Vec::new();
                if i % CHECK_EVERY == 0 && cancel.load(Ordering::Relaxed) {
                    return found;
                }
                let (a, b) = at[i];
                // Sides running the other way start near b and end near a. Each pair is found
                // from both of its sides; the lower one keeps it.
                grid.near(b, tolerance, |j| {
                    let j = j as usize;
                    if j <= i || sides[i] / 3 == sides[j] / 3 {
                        return;
                    }
                    let (c, d) = at[j];
                    let (ad, bc) = (distance2(a, d), distance2(b, c));
                    if ad <= limit && bc <= limit && dot(sub(b, a), sub(c, d)) > 0.0 {
                        found.push(Pair {
                            cost: ad.max(bc),
                            same: false,
                            i: i as u32,
                            j: j as u32,
                        });
                    }
                });
                if ignore_orientation {
                    grid.near(a, tolerance, |j| {
                        let j = j as usize;
                        if j <= i || sides[i] / 3 == sides[j] / 3 {
                            return;
                        }
                        let (c, d) = at[j];
                        let (ac, bd) = (distance2(a, c), distance2(b, d));
                        if ac <= limit && bd <= limit && dot(sub(b, a), sub(d, c)) > 0.0 {
                            found.push(Pair {
                                cost: ac.max(bd),
                                same: true,
                                i: i as u32,
                                j: j as u32,
                            });
                        }
                    });
                }
                found
            })
            .collect();
        control.check()?;
        pairs.par_sort_unstable_by(|x, y| {
            x.cost
                .total_cmp(&y.cost)
                .then(x.same.cmp(&y.same))
                .then(x.i.cmp(&y.i))
                .then(x.j.cmp(&y.j))
        });
        let mut used = vec![false; sides.len()];
        let mut joined = 0;
        for (n, pair) in pairs.iter().enumerate() {
            if n % CHECK_EVERY == 0 {
                control.check()?;
            }
            let (i, j) = (pair.i as usize, pair.j as usize);
            if !used[i] && !used[j] && self.try_join(sides[i], sides[j], pair.same) {
                used[i] = true;
                used[j] = true;
                joined += 1;
            }
        }
        Ok(joined)
    }

    /// Joins border sides `s` and `r` by merging their ends (a with d and b with c, or a with c
    /// and b with d when they run the same way) if the triangles around allow it (module doc).
    fn try_join(&mut self, s: u32, r: u32, same: bool) -> bool {
        let mut scratch = std::mem::take(&mut self.scratch);
        let merges = self.check_join(s, r, same, &mut scratch);
        if let Some([(x1, y1), (x2, y2)]) = merges {
            for &t in &scratch.collapsing {
                self.collapsed[t as usize] = true;
            }
            self.union(x1, y1);
            self.union(x2, y2);
        }
        self.scratch = scratch;
        merges.is_some()
    }

    /// The two merges (pairs of root vertices) that join border sides `s` and `r`, when the
    /// triangles around allow them; the triangles they would collapse are left in
    /// `scratch.collapsing`.
    fn check_join(
        &self,
        s: u32,
        r: u32,
        same: bool,
        scratch: &mut Scratch,
    ) -> Option<[(u32, u32); 2]> {
        if self.collapsed[(s / 3) as usize] || self.collapsed[(r / 3) as usize] {
            return None;
        }
        let (a, b) = self.ends(s);
        let (c, d) = self.ends(r);
        let [(x1, y1), (x2, y2)] = if same {
            [(a, c), (b, d)]
        } else {
            [(a, d), (b, c)]
        };
        // The two groups must stay apart, or one of the sides would shrink to a point.
        if x1 == x2 || x1 == y2 || y1 == x2 || y1 == y2 {
            return None;
        }
        let limit = self.tolerance * self.tolerance;
        if distance2(self.at(x1), self.at(y1)) > limit
            || distance2(self.at(x2), self.at(y2)) > limit
        {
            return None;
        }
        let (n1, n2) = (x1.min(y1), x2.min(y2));
        let merged = |v: u32| {
            if v == x1 || v == y1 {
                n1
            } else if v == x2 || v == y2 {
                n2
            } else {
                v
            }
        };
        let Scratch {
            triangles,
            collapsing,
            sides,
            faces,
        } = scratch;
        self.gather_around(&[x1, y1, x2, y2], triangles);
        collapsing.clear();
        // For each side that ends at a merged group: its edge after and before, and whether it
        // runs from the lower vertex. For each triangle that stays: its corners after and before.
        sides.clear();
        faces.clear();
        for &t in triangles.iter() {
            let before = self.corners(t);
            let after = before.map(merged);
            let was_flat = repeats(before);
            let flat = repeats(after);
            if !was_flat && flat {
                if t == s / 3 || t == r / 3 {
                    return None;
                }
                collapsing.push(t);
                continue;
            }
            if !was_flat && after != before {
                let was = triangle_normal(before.map(|v| self.at(v)));
                let now = triangle_normal(after.map(|v| self.at(v)));
                if dot(was, was) > 0.0 && dot(was, now) <= 0.0 {
                    return None;
                }
            }
            for k in 0..3 {
                let (p, q) = (after[k], after[(k + 1) % 3]);
                if p != q && (p == n1 || p == n2 || q == n1 || q == n2) {
                    let old = edge_key(before[k], before[(k + 1) % 3]);
                    sides.push((edge_key(p, q), old, p < q));
                }
            }
            if !flat {
                faces.push((sorted(after), sorted(before)));
            }
        }
        // An edge that was one edge before is as it was. An edge that several edges become must
        // end up with exactly two sides, running opposite ways unless orientation is ignored.
        sides.sort_unstable();
        for group in sides.chunk_by(|x, y| x.0 == y.0) {
            if group.iter().all(|x| x.1 == group[0].1) {
                continue;
            }
            if group.len() != 2 || (!self.ignore_orientation && group[0].2 == group[1].2) {
                return None;
            }
        }
        // No two triangles may become one.
        faces.sort_unstable();
        if faces
            .windows(2)
            .any(|w| w[0].0 == w[1].0 && w[0].1 != w[1].1)
        {
            return None;
        }
        Some([(x1, y1), (x2, y2)])
    }

    /// Puts the triangles (not collapsed) around the groups of the given root border vertices
    /// into `out`, ascending and each once.
    fn gather_around(&self, roots: &[u32], out: &mut Vec<u32>) {
        out.clear();
        for (i, &v) in roots.iter().enumerate() {
            if !roots[..i].contains(&v) {
                let list = &self.around[self.local[v as usize] as usize];
                out.extend(list.iter().filter(|&&t| !self.collapsed[t as usize]));
            }
        }
        out.sort_unstable();
        out.dedup();
    }

    /// Merges the groups of roots `x` and `y`; the lower root stays.
    fn union(&mut self, x: u32, y: u32) {
        if x == y {
            return;
        }
        let (keep, gone) = (x.min(y), x.max(y));
        let (kept, moved) = (self.local[keep as usize], self.local[gone as usize]);
        let mut member = moved;
        let mut last = moved;
        while member != NONE {
            self.root[self.border_vertices[member as usize] as usize] = keep;
            last = member;
            member = self.next_member[member as usize];
        }
        self.next_member[last as usize] = self.next_member[kept as usize];
        self.next_member[kept as usize] = moved;
        let list = std::mem::take(&mut self.around[moved as usize]);
        let collapsed = &self.collapsed;
        let into = &mut self.around[kept as usize];
        into.extend(list);
        into.retain(|&t| !collapsed[t as usize]);
        into.sort_unstable();
        into.dedup();
    }

    /// One round of T-junction splits (module doc); the number of vertices put into sides.
    ///
    /// A border vertex v goes into a border side p→q when it lies within the tolerance of the
    /// side's inside (farther than the tolerance from p and q, which a pair join handles) and the
    /// vertex's own border runs along the side: one of its border sides ends within the tolerance
    /// of the side's line, toward p for the side leaving v and toward q for the side entering it
    /// (so the lips run opposite ways; either way with Ignore orientation). Nearest first; a side
    /// can take several vertices in one round, each into the piece of the side it lies on.
    fn split_t_junctions(&mut self, border: &[u32], control: &mut Control) -> RepairResult<usize> {
        let sides = self.sides_taking_part(border);
        if sides.len() < 2 {
            return Ok(0);
        }
        let tolerance = self.tolerance;
        let limit = tolerance * tolerance;
        let ends: Vec<(u32, u32)> = sides.iter().map(|&s| self.ends(s)).collect();
        let at: Vec<(P3, P3)> = ends
            .iter()
            .map(|&(a, b)| (self.at(a), self.at(b)))
            .collect();
        // The border sides leaving and entering each vertex, to follow the vertex's own border.
        let mut leaving: Vec<(u32, u32)> = ends
            .iter()
            .enumerate()
            .map(|(i, &(a, _))| (a, i as u32))
            .collect();
        let mut entering: Vec<(u32, u32)> = ends
            .iter()
            .enumerate()
            .map(|(i, &(_, b))| (b, i as u32))
            .collect();
        leaving.sort_unstable();
        entering.sort_unstable();
        let boxes: Vec<Aabb> = at
            .iter()
            .map(|&(a, b)| {
                let mut lo = [0.0; 3];
                let mut hi = [0.0; 3];
                for k in 0..3 {
                    lo[k] = a[k].min(b[k]) - tolerance;
                    hi[k] = a[k].max(b[k]) + tolerance;
                }
                [lo, hi]
            })
            .collect();
        let bvh = Bvh::new(&boxes);
        let mut vertices: Vec<u32> = ends.iter().flat_map(|&(a, b)| [a, b]).collect();
        vertices.sort_unstable();
        vertices.dedup();
        let cancel = control.cancel_flag();
        // Each vertex looks for its sides on its own, on every core; the sort below fixes the
        // order.
        let mut junctions: Vec<Junction> = vertices
            .par_iter()
            .enumerate()
            .flat_map_iter(|(n, &v)| {
                let mut found = Vec::new();
                if n % CHECK_EVERY == 0 && cancel.load(Ordering::Relaxed) {
                    return found;
                }
                let pv = self.at(v);
                bvh.query(&[pv, pv], |i| {
                    let (p, q) = ends[i as usize];
                    if v == p || v == q || self.corners(sides[i as usize] / 3).contains(&v) {
                        return;
                    }
                    let (pp, pq) = at[i as usize];
                    let (t, distance) = along(pv, pp, pq);
                    if t <= 0.0
                        || t >= 1.0
                        || distance > limit
                        || distance2(pv, pp) <= limit
                        || distance2(pv, pq) <= limit
                    {
                        return;
                    }
                    if self.follows(v, t, [pp, pq], &ends, &leaving, true)
                        || self.follows(v, t, [pp, pq], &ends, &entering, false)
                    {
                        found.push(Junction {
                            distance,
                            vertex: v,
                            side: i,
                            t,
                        });
                    }
                });
                found
            })
            .collect();
        control.check()?;
        junctions.sort_unstable_by(|x, y| {
            x.distance
                .total_cmp(&y.distance)
                .then(x.vertex.cmp(&y.vertex))
                .then(x.side.cmp(&y.side))
        });
        // Per side split this round: its pieces in order along it, as (where the piece starts,
        // its side, its start vertex, its end vertex).
        let mut pieces: HashMap<u32, Vec<(f64, u32, u32, u32)>> = HashMap::new();
        let mut placed = vec![false; self.border_vertices.len()];
        let mut split = 0;
        for (n, junction) in junctions.iter().enumerate() {
            if n % CHECK_EVERY == 0 {
                control.check()?;
            }
            let v = junction.vertex;
            if placed[self.local[v as usize] as usize] {
                continue;
            }
            let i = junction.side;
            let list = pieces.entry(i).or_insert_with(|| {
                let (p, q) = ends[i as usize];
                vec![(0.0, sides[i as usize], p, q)]
            });
            let k = list.partition_point(|piece| piece.0 < junction.t) - 1;
            let (_, side, from, to) = list[k];
            // A split of another side of the same triangle may have moved this side elsewhere.
            if self.ends(side) != (from, to) {
                continue;
            }
            if let Some(piece) = self.try_split(side, v) {
                list[k].3 = v;
                list.insert(k + 1, (junction.t, piece * 3, v, to));
                placed[self.local[v as usize] as usize] = true;
                split += 1;
            }
        }
        Ok(split)
    }

    /// Whether border vertex `v`, which lies inside the border side from p to q at `t` (0 at p, 1
    /// at q), has its own border running along that side: one of its sides in `list` (those
    /// leaving v, or those entering it, as positions in `ends`) ends within the tolerance of the
    /// side's line, the right way round (toward p for a side leaving v, `toward_start`; either way
    /// with Ignore orientation), and overlaps the side by more than tolerance-sized noise. Border
    /// edges that merely continue one another along a line meet end to end, not side by side.
    fn follows(
        &self,
        v: u32,
        t: f64,
        [p, q]: [P3; 2],
        ends: &[(u32, u32)],
        list: &[(u32, u32)],
        toward_start: bool,
    ) -> bool {
        let tolerance = self.tolerance;
        let limit = tolerance * tolerance;
        let length = distance2(p, q).sqrt();
        let slack = tolerance / length;
        let first = list.partition_point(|&(x, _)| x < v);
        list[first..]
            .iter()
            .take_while(|&&(x, _)| x == v)
            .any(|&(_, own)| {
                let (a, b) = ends[own as usize];
                let other = if a == v { b } else { a };
                let (u, off) = along(self.at(other), p, q);
                let overlap = (u.max(t).min(1.0) - u.min(t).max(0.0)) * length;
                off <= limit
                    && u != t
                    && (self.ignore_orientation || (u < t) == toward_start)
                    && (overlap > 2.0 * tolerance || (-slack..=1.0 + slack).contains(&u))
            })
    }

    /// Puts border vertex `v` into `side` p→q of triangle (p, q, y): the triangle becomes (p, v,
    /// y) and a new piece (v, q, y) follows, whose first side v→q is returned as its triangle. None
    /// when v no longer lies inside the side away from its ends, when either new triangle would
    /// face the other way, or when the sides already at v allow no consistent join: v–y must be a
    /// new edge, and v–p and v–q may each hold one side, running against the new one unless
    /// orientation is ignored.
    fn try_split(&mut self, side: u32, v: u32) -> Option<u32> {
        let t = side / 3;
        let k = (side % 3) as usize;
        let c = self.corners(t);
        let (p, q, y) = (c[k], c[(k + 1) % 3], c[(k + 2) % 3]);
        if v == p || v == q || v == y {
            return None;
        }
        let limit = self.tolerance * self.tolerance;
        let (pp, pq, py, pv) = (self.at(p), self.at(q), self.at(y), self.at(v));
        let (u, _) = along(pv, pp, pq);
        if u <= 0.0 || u >= 1.0 || distance2(pv, pp) <= limit || distance2(pv, pq) <= limit {
            return None;
        }
        let n = normal(pp, pq, py);
        if dot(normal(pp, pv, py), n) <= 0.0 || dot(normal(pv, pq, py), n) <= 0.0 {
            return None;
        }
        // Sides already on v–p and v–q, by direction.
        let (mut v_to_p, mut p_to_v, mut v_to_q, mut q_to_v) = (0, 0, 0, 0);
        for &other in &self.around[self.local[v as usize] as usize] {
            if self.collapsed[other as usize] {
                continue;
            }
            let cu = self.corners(other);
            for kk in 0..3 {
                match (cu[kk], cu[(kk + 1) % 3]) {
                    (a, b) if (a == v && b == y) || (a == y && b == v) => return None,
                    (a, b) if a == v && b == p => v_to_p += 1,
                    (a, b) if a == p && b == v => p_to_v += 1,
                    (a, b) if a == v && b == q => v_to_q += 1,
                    (a, b) if a == q && b == v => q_to_v += 1,
                    _ => {}
                }
            }
        }
        // The new side p→v joins a side v→p; the new side v→q joins a side q→v.
        let strict = !self.ignore_orientation;
        if v_to_p + p_to_v > 1 || (strict && p_to_v > 0) {
            return None;
        }
        if v_to_q + q_to_v > 1 || (strict && v_to_q > 0) {
            return None;
        }
        let piece = self.triangles.len() as u32;
        self.triangles[t as usize][(k + 1) % 3] = v;
        self.triangles.push([v, q, y]);
        self.origin.push(self.origin[t as usize]);
        self.collapsed.push(false);
        self.region.push(piece);
        let lv = self.local[v as usize] as usize;
        self.around[lv].extend_from_slice(&[t, piece]);
        let lq = self.local[q as usize] as usize;
        self.around[lq].retain(|&x| x != t);
        self.around[lq].push(piece);
        if let Some(&ly) = self.local.get(y as usize).filter(|&&l| l != NONE) {
            self.around[ly as usize].push(piece);
        }
        Some(piece)
    }

    /// The stitched mesh (collapsed triangles dropped, unused vertices removed) and its origin.
    fn finish(self) -> (Mesh, Origin) {
        let input = self.mesh.triangle_count();
        let mut indices = Vec::with_capacity(self.triangles.len() * 3);
        let mut origin = Vec::with_capacity(self.triangles.len());
        for (t, c) in self.triangles.iter().enumerate() {
            if !self.collapsed[t] {
                indices.extend(c.map(|v| self.root[v as usize]));
                origin.push(self.origin[t]);
            }
        }
        // Merging alone keeps every triangle where it was.
        let origin = if self.triangles.len() == input && origin.len() == input {
            Origin::Same
        } else {
            Origin::Map(origin)
        };
        (compact(&self.mesh.positions, indices), origin)
    }
}

/// Points sorted into cubic cells, to find the points near a position without comparing every
/// pair.
struct Grid {
    cell: f64,
    /// Point numbers ordered by cell.
    ids: Vec<u32>,
    /// Each occupied cell's run of `ids`.
    runs: HashMap<[i64; 3], (u32, u32), BuildHasherDefault<CellHasher>>,
}

/// A quick hash for cell numbers (std's default hasher, built to resist attacks, took most of the
/// search's time): a multiply and rotate per number, as rustc's FxHash does.
#[derive(Default)]
struct CellHasher(u64);

impl Hasher for CellHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(b as u64);
        }
    }

    fn write_u64(&mut self, x: u64) {
        self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }

    fn write_i64(&mut self, x: i64) {
        self.write_u64(x as u64);
    }

    fn write_usize(&mut self, x: usize) {
        self.write_u64(x as u64);
    }
}

impl Grid {
    fn new(points: &[P3], cell: f64) -> Grid {
        let mut keyed: Vec<([i64; 3], u32)> = points
            .iter()
            .enumerate()
            .map(|(i, &p)| (cell_of(p, cell), i as u32))
            .collect();
        keyed.par_sort_unstable();
        let mut runs = HashMap::default();
        let mut start = 0u32;
        for group in keyed.chunk_by(|a, b| a.0 == b.0) {
            let end = start + group.len() as u32;
            runs.insert(group[0].0, (start, end));
            start = end;
        }
        Grid {
            cell,
            ids: keyed.into_iter().map(|(_, i)| i).collect(),
            runs,
        }
    }

    /// Calls `f(i)` for every point within `radius` of `p`, and some a little farther.
    fn near(&self, p: P3, radius: f64, mut f: impl FnMut(u32)) {
        // A hair wider than the radius, so rounding cannot skip a cell holding a point within it.
        let reach = radius * (1.0 + 1e-9) + self.cell * 1e-9;
        let lo = cell_of(p.map(|x| x - reach), self.cell);
        let hi = cell_of(p.map(|x| x + reach), self.cell);
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    if let Some(&(start, end)) = self.runs.get(&[x, y, z]) {
                        for &i in &self.ids[start as usize..end as usize] {
                            f(i);
                        }
                    }
                }
            }
        }
    }
}

fn cell_of(p: P3, cell: f64) -> [i64; 3] {
    p.map(|x| (x / cell).floor() as i64)
}

/// Where `p` lies along the line from `a` to `b` (0 at a, 1 at b) and its squared distance from
/// that line. A zero-length line puts every point at 0, at its distance from `a`.
fn along(p: P3, a: P3, b: P3) -> (f64, f64) {
    let d = sub(b, a);
    let length = dot(d, d);
    let t = if length > 0.0 {
        dot(sub(p, a), d) / length
    } else {
        0.0
    };
    (t, distance2(p, add(a, scale(d, t))))
}

#[inline]
fn distance2(a: P3, b: P3) -> f64 {
    let d = sub(a, b);
    dot(d, d)
}

fn triangle_normal([a, b, c]: [P3; 3]) -> P3 {
    cross(sub(b, a), sub(c, a))
}

/// The lower and higher vertex of an edge in one number.
#[inline]
fn edge_key(p: u32, q: u32) -> u64 {
    ((p.min(q) as u64) << 32) | p.max(q) as u64
}

fn repeats([a, b, c]: [u32; 3]) -> bool {
    a == b || b == c || c == a
}

fn sorted(mut t: [u32; 3]) -> [u32; 3] {
    t.sort_unstable();
    t
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    use super::*;
    use crate::mesh::make_box;
    use crate::ops::concat;
    use crate::repair::status::{RepairStatus, analyze};
    use crate::repair::topology::{Link, boundary_loops, components};
    use crate::repair::{RepairError, remap_selection, silent};

    const TOLERANCE: StitchParams = StitchParams {
        tolerance: 0.01,
        ignore_orientation: false,
    };

    const ANY_WAY: StitchParams = StitchParams {
        tolerance: 0.01,
        ignore_orientation: true,
    };

    fn stitch(mesh: &Mesh, params: &StitchParams) -> Outcome {
        silent(|c| run(mesh, &[], params, c)).unwrap()
    }

    /// Stitches, checks what every stitch must leave (no new non-manifold edges, every vertex an
    /// input position, the origin map) and that a second run changes nothing, and returns the
    /// first run's outcome.
    fn stitch_checked(mesh: &Mesh, selection: &[u32], params: &StitchParams) -> Outcome {
        let out = silent(|c| run(mesh, selection, params, c)).unwrap();
        let before = status(mesh);
        let after = status(&out.mesh);
        assert!(
            after.non_manifold_edges <= before.non_manifold_edges,
            "non-manifold edges {} -> {}",
            before.non_manifold_edges,
            after.non_manifold_edges
        );
        if out.changed {
            assert!(after.border_edges < before.border_edges);
        } else {
            assert_eq!(out.mesh, *mesh);
            assert_eq!(out.origin, Origin::Same);
        }
        let inputs: Vec<[u32; 3]> = mesh
            .positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| p.map(f32::to_bits))
            .collect();
        for p in out.mesh.positions.as_chunks::<3>().0 {
            assert!(inputs.contains(&p.map(f32::to_bits)), "new position {p:?}");
        }
        match &out.origin {
            Origin::Same => assert_eq!(out.mesh.triangle_count(), mesh.triangle_count()),
            Origin::Map(map) => {
                assert_eq!(map.len(), out.mesh.triangle_count());
                assert!(map.iter().all(|&t| (t as usize) < mesh.triangle_count()));
            }
        }
        let selection = remap_selection(selection, &out.origin, out.mesh.triangle_count());
        let again = silent(|c| run(&out.mesh, &selection, params, c)).unwrap();
        assert!(!again.changed, "the second run changed: {}", again.message);
        assert_eq!(again.mesh, out.mesh);
        out
    }

    fn status(mesh: &Mesh) -> RepairStatus {
        silent(|c| analyze(mesh, c)).unwrap().status
    }

    /// make_box's faces come in the order −z, +z, −y, +y, +x, −x, two triangles each.
    fn without_face(mut mesh: Mesh, face: usize) -> Mesh {
        mesh.indices.drain(face * 6..face * 6 + 6);
        mesh
    }

    fn moved(mut mesh: Mesh, d: [f32; 3]) -> Mesh {
        for p in mesh.positions.as_chunks_mut::<3>().0 {
            for k in 0..3 {
                p[k] += d[k];
            }
        }
        mesh
    }

    fn inside_out(mut mesh: Mesh) -> Mesh {
        for t in mesh.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
        mesh
    }

    fn left_half() -> Mesh {
        without_face(make_box([0.0; 3], [10.0, 20.0, 20.0]), 4)
    }

    fn right_half(gap: f32) -> Mesh {
        moved(
            without_face(make_box([10.0, 0.0, 0.0], [20.0; 3]), 5),
            [gap, 0.0, 0.0],
        )
    }

    /// The 20 mm cube cut at x = 10 into two open halves, the right one moved `gap` mm along x:
    /// the cube-crack fixture for a gap of 0.005.
    fn crack(gap: f32) -> Mesh {
        concat(&[&left_half(), &right_half(gap)]).unwrap()
    }

    /// The mesh with every border side's triangle split into `parts` along that side (a fan from
    /// the opposite corner), so the border gets `parts - 1` more vertices per edge.
    fn subdivide_border(mesh: &Mesh, parts: u32) -> Mesh {
        let edges = Edges::new(mesh);
        let mut out = mesh.clone();
        let mut done = vec![false; mesh.triangle_count()];
        let mut replaced = Vec::new();
        for e in 0..edges.len() as u32 {
            if edges.degree(e) != 1 {
                continue;
            }
            let side = edges.sides(e)[0];
            let t = (side / 3) as usize;
            assert!(!done[t], "two border sides on triangle {t}");
            done[t] = true;
            let k = (side % 3) as usize;
            let c = [0, 1, 2].map(|i| mesh.indices[t * 3 + i]);
            let (p, q, y) = (c[k], c[(k + 1) % 3], c[(k + 2) % 3]);
            let (pp, pq) = (mesh.point(p), mesh.point(q));
            let mut chain = vec![p];
            for i in 1..parts {
                let f = i as f64 / parts as f64;
                let m = add(pp, scale(sub(pq, pp), f));
                chain.push(out.vertex_count() as u32);
                out.positions
                    .extend_from_slice(&[m[0] as f32, m[1] as f32, m[2] as f32]);
            }
            chain.push(q);
            replaced.push(t);
            for w in chain.windows(2) {
                out.indices.extend_from_slice(&[w[0], w[1], y]);
            }
        }
        let keep: Vec<bool> = (0..out.triangle_count())
            .map(|t| t >= mesh.triangle_count() || !replaced.contains(&t))
            .collect();
        crate::repair::topology::keep_triangles(&out, &keep).0
    }

    fn cm3(volume: f64) -> String {
        format!("{:.3}", volume / 1000.0)
    }

    #[test]
    fn a_closed_mesh_is_unchanged_after_one_scan() {
        let cube = make_box([0.0; 3], [20.0; 3]);
        let out = stitch_checked(&cube, &[], &TOLERANCE);
        assert!(!out.changed);
        assert_eq!(out.message, "No border edges to stitch.");
        // Two closed cubes touching face to face have no border edges either.
        let pair = concat(&[&cube, &make_box([20.0, 0.0, 0.0], [40.0, 20.0, 20.0])]).unwrap();
        assert!(!stitch(&pair, &ANY_WAY).changed);
        assert!(!stitch(&Mesh::default(), &TOLERANCE).changed);
    }

    #[test]
    fn the_crack_closes_into_one_closed_shell() {
        let mesh = crack(0.005);
        let before = status(&mesh);
        assert_eq!(
            (before.shells, before.holes, before.border_edges),
            (2, 2, 8)
        );
        let out = stitch_checked(&mesh, &[], &TOLERANCE);
        assert!(out.changed);
        assert_eq!(out.message, "Stitched 4 edges.");
        let s = status(&out.mesh);
        assert_eq!((s.shells, s.border_edges, s.holes), (1, 0, 0), "{s:?}");
        assert!(s.closed && s.oriented, "{s:?}");
        // The left half's vertices come first and keep their places; the right half's open face
        // moves onto them, so the right half stays 0.005 mm longer.
        assert_eq!(cm3(s.volume), "8.002");
        assert_eq!(out.origin, Origin::Same);
        assert_eq!(out.mesh.vertex_count(), 12);
        // The same with Ignore orientation.
        let any = stitch_checked(&mesh, &[], &ANY_WAY);
        assert_eq!(any.mesh, out.mesh);
    }

    #[test]
    fn a_gap_wider_than_the_tolerance_stays_open() {
        let mesh = crack(0.02);
        let out = stitch_checked(&mesh, &[], &TOLERANCE);
        assert!(!out.changed);
        assert_eq!(out.message, "No border edges close enough to stitch.");
        let wider = StitchParams {
            tolerance: 0.03,
            ..TOLERANCE
        };
        let out = stitch_checked(&mesh, &[], &wider);
        assert!(status(&out.mesh).closed);
    }

    #[test]
    fn halves_facing_opposite_ways_join_only_when_orientation_is_ignored() {
        let mesh = concat(&[&left_half(), &inside_out(right_half(0.005))]).unwrap();
        let out = stitch_checked(&mesh, &[], &TOLERANCE);
        assert!(!out.changed);
        let out = stitch_checked(&mesh, &[], &ANY_WAY);
        assert_eq!(out.message, "Stitched 4 edges.");
        let s = status(&out.mesh);
        // Two shells until the halves agree: shells part where the faces disagree.
        assert_eq!((s.shells, s.border_edges, s.holes), (2, 0, 0));
        // Closed, but one half faces the other way: Fix flipped triangles turns it next.
        assert!(s.closed && !s.oriented);
        assert_eq!(s.inverted_triangles, 10);
    }

    #[test]
    fn t_junctions_line_up_lips_with_different_vertices() {
        // The left lip has the edges' midpoints, the right lip does not; then the other way
        // round; then thirds against halves.
        let cases = [
            (subdivide_border(&left_half(), 2), right_half(0.005)),
            (left_half(), subdivide_border(&right_half(0.005), 2)),
            (
                subdivide_border(&left_half(), 3),
                subdivide_border(&right_half(0.004), 2),
            ),
        ];
        for (n, (left, right)) in cases.iter().enumerate() {
            let mesh = concat(&[left, right]).unwrap();
            let out = stitch_checked(&mesh, &[], &TOLERANCE);
            let s = status(&out.mesh);
            assert_eq!(
                (s.shells, s.border_edges, s.holes),
                (1, 0, 0),
                "case {n}: {s:?}"
            );
            assert!(s.closed && s.oriented, "case {n}: {s:?}");
            assert!(out.message.starts_with("Stitched "), "case {n}");
            // The split pieces come last and keep their triangle's origin.
            let Origin::Map(map) = &out.origin else {
                panic!("case {n}: splits change the triangle list");
            };
            assert!(map.len() > mesh.triangle_count());
            assert!(
                map[..mesh.triangle_count()]
                    .iter()
                    .enumerate()
                    .all(|(t, &o)| o == t as u32)
            );
        }
    }

    /// A flat sheet cut along y = 0 into a lower half with vertices at x = 0, 3, 6, 9, 12 on the
    /// cut and an upper half with vertices at x = 0, 4, 8, 12, lifted by `gap`; with `welded` the
    /// two halves share the cut's end vertices.
    fn cut_sheet(gap: f32, welded: bool) -> Mesh {
        let mut positions: Vec<f32> = Vec::new();
        let mut point = |x: f32, y: f32| {
            positions.extend_from_slice(&[x, y, 0.0]);
            (positions.len() / 3 - 1) as u32
        };
        let low: Vec<u32> = (0..5).map(|i| point(3.0 * i as f32, 0.0)).collect();
        let (b0, b1) = (point(0.0, -5.0), point(12.0, -5.0));
        let mut high: Vec<u32> = (0..4).map(|j| point(4.0 * j as f32, gap)).collect();
        let (t0, t1) = (point(0.0, 5.0 + gap), point(12.0, 5.0 + gap));
        if welded {
            high[0] = low[0];
            high[3] = low[4];
        }
        // Both halves face +z.
        let lower = [
            [b0, low[1], low[0]],
            [b0, low[2], low[1]],
            [b0, b1, low[2]],
            [b1, low[3], low[2]],
            [b1, low[4], low[3]],
        ];
        let upper = [
            [high[0], high[1], t0],
            [high[1], high[2], t0],
            [high[2], t1, t0],
            [high[2], high[3], t1],
        ];
        let indices = lower.iter().chain(&upper).flatten().copied().collect();
        let mesh = Mesh { positions, indices };
        crate::repair::topology::keep_triangles(&mesh, &vec![true; mesh.triangle_count()]).0
    }

    /// The total length of the border edges.
    fn border_length(mesh: &Mesh) -> f64 {
        let edges = Edges::new(mesh);
        (0..edges.len() as u32)
            .filter(|&e| edges.degree(e) == 1)
            .map(|e| {
                let [a, b] = edges.ends(e);
                distance2(mesh.point(a), mesh.point(b)).sqrt()
            })
            .sum()
    }

    #[test]
    fn a_crack_between_different_tessellations_closes_from_both_lips() {
        for (gap, welded) in [(0.003, false), (0.0, true), (0.0, false), (0.006, false)] {
            let mesh = cut_sheet(gap, welded);
            assert!(status(&mesh).oriented);
            let out = stitch_checked(&mesh, &[], &TOLERANCE);
            let s = status(&out.mesh);
            // Only the sheet's outline is left open.
            assert_eq!(
                (s.shells, s.holes, s.non_manifold_edges),
                (1, 1, 0),
                "{gap}: {s:?}"
            );
            assert!(s.oriented, "{gap}: {s:?}");
            let outline = 2.0 * 12.0 + 2.0 * (10.0 + gap as f64);
            assert!(
                (border_length(&out.mesh) - outline).abs() < 0.02,
                "gap {gap}: border {} against {outline}",
                border_length(&out.mesh)
            );
        }
    }

    #[test]
    fn a_slit_zips_shut_from_its_closed_end() {
        // Two triangles meeting at the tip, their facing edges 0.008 mm apart at the far end.
        let mesh = Mesh {
            positions: vec![
                0.0, 0.0, 0.0, // 0 tip
                5.0, 5.0, 0.0, // 1
                10.0, 0.004, 0.0, // 2 upper lip end
                10.0, -0.004, 0.0, // 3 lower lip end
                5.0, -5.0, 0.0, // 4
            ],
            indices: vec![0, 1, 2, 0, 3, 4],
        };
        // Both triangles face -z: (1 - 0) × (2 - 0) points down.
        let out = stitch_checked(&mesh, &[], &TOLERANCE);
        assert_eq!(out.message, "Stitched 1 edge.");
        let s = status(&out.mesh);
        assert_eq!((s.border_edges, s.holes, s.shells), (4, 1, 1));
        assert!(s.oriented);
        assert_eq!(out.mesh.vertex_count(), 4);
    }

    #[test]
    fn sides_of_one_triangle_are_never_joined() {
        // A needle whose short corner pair lies within the tolerance, and a speck smaller than it.
        for positions in [
            vec![0.0, 0.0, 0.0, 5.0, 0.0, 0.0, 0.004, 0.003, 0.0],
            vec![0.0, 0.0, 0.0, 0.004, 0.0, 0.0, 0.0, 0.004, 0.0],
        ] {
            let mesh = Mesh {
                positions,
                indices: vec![0, 1, 2],
            };
            assert!(!stitch_checked(&mesh, &[], &ANY_WAY).changed);
        }
    }

    #[test]
    fn a_third_lip_never_makes_a_non_manifold_edge_and_merges_never_chain() {
        // Three open sheets along the x axis: A (y ≥ 0) ends at y = 0, B (y ≤ 0.006) ends at
        // y = 0.006, and C stands up (z ≥ 0) from y = 0.012. A and B fit, and so do B and C, but A
        // and C are 0.012 apart: whichever pair joins first, the third sheet stays where it was.
        let quad = |corners: [[f32; 3]; 4]| Mesh {
            positions: corners.into_iter().flatten().collect(),
            indices: vec![0, 1, 2, 0, 2, 3],
        };
        let a = quad([
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 5.0, 0.0],
            [0.0, 5.0, 0.0],
        ]);
        let b = quad([
            [10.0, 0.006, 0.0],
            [0.0, 0.006, 0.0],
            [0.0, -5.0, 0.0],
            [10.0, -5.0, 0.0],
        ]);
        let c = quad([
            [0.0, 0.012, 0.0],
            [10.0, 0.012, 0.0],
            [10.0, 0.012, 5.0],
            [0.0, 0.012, 5.0],
        ]);
        let corners_of = |m: &Mesh| -> Vec<[u32; 3]> {
            m.positions
                .as_chunks::<3>()
                .0
                .iter()
                .map(|p| p.map(f32::to_bits))
                .collect()
        };
        for params in [TOLERANCE, ANY_WAY] {
            for order in [[&a, &b, &c], [&c, &b, &a], [&b, &c, &a]] {
                let mesh = concat(&order).unwrap();
                let out = stitch_checked(&mesh, &[], &params);
                assert_eq!(out.message, "Stitched 1 edge.");
                let s = status(&out.mesh);
                assert_eq!((s.non_manifold_edges, s.shells), (0, 2), "{s:?}");
                // Two of the joined sheets' corners merged; the sheet left out, and the joined
                // sheet that came first, keep all their corners where they were.
                assert_eq!(out.mesh.vertex_count(), 10);
                let left = corners_of(&out.mesh);
                let whole = [&a, &b, &c]
                    .iter()
                    .filter(|sheet| corners_of(sheet).iter().all(|p| left.contains(p)))
                    .count();
                assert_eq!(whole, 2);
            }
        }
    }

    #[test]
    fn a_selection_limits_the_stitch_to_its_border_edges() {
        let mesh = crack(0.005);
        // The left half alone: its lip has nothing selected to join.
        let left: Vec<u32> = (0..10).collect();
        assert!(!stitch_checked(&mesh, &left, &TOLERANCE).changed);
        // Triangles away from the border.
        let out = stitch_checked(&mesh, &[8, 9], &TOLERANCE);
        assert!(!out.changed);
        assert_eq!(
            out.message,
            "The selected triangles have no border edges to stitch."
        );
        // Everything selected stitches everything.
        let all: Vec<u32> = (0..20).collect();
        let out = stitch_checked(&mesh, &all, &TOLERANCE);
        assert!(status(&out.mesh).closed);
        // One triangle on each lip of the bottom seam: the left half's -z triangle (0, 2, 1) holds
        // its border side 2→1, the right half's (0, 3, 2) holds 0→3. Only that seam closes.
        let left_bottom = 0;
        let right_bottom = 10 + 1;
        let out = stitch_checked(&mesh, &[left_bottom, right_bottom], &TOLERANCE);
        assert_eq!(out.message, "Stitched 1 edge.");
        assert_eq!(status(&out.mesh).border_edges, 6);
        assert_eq!(out.origin, Origin::Same);
        // T-junctions too: the subdivided left lip alone has nothing to go into.
        let mesh = concat(&[&subdivide_border(&left_half(), 2), &right_half(0.005)]).unwrap();
        let left = (0..14).collect::<Vec<u32>>();
        assert!(!stitch_checked(&mesh, &left, &TOLERANCE).changed);
        let all: Vec<u32> = (0..mesh.triangle_count() as u32).collect();
        let out = stitch_checked(&mesh, &all, &TOLERANCE);
        assert!(status(&out.mesh).closed);
        // The split pieces of selected triangles are selected too.
        let selected = remap_selection(&all, &out.origin, out.mesh.triangle_count());
        assert_eq!(selected.len(), out.mesh.triangle_count());
    }

    #[test]
    fn a_zero_tolerance_joins_exactly_touching_lips_only() {
        let exact = StitchParams {
            tolerance: 0.0,
            ignore_orientation: false,
        };
        let out = stitch_checked(&crack(0.0), &[], &exact);
        assert!(status(&out.mesh).closed);
        assert!(!stitch(&crack(0.005), &exact).changed);
        // A negative or NaN tolerance counts as 0.
        for tolerance in [-1.0, f64::NAN] {
            let odd = StitchParams {
                tolerance,
                ignore_orientation: false,
            };
            assert!(status(&stitch(&crack(0.0), &odd).mesh).closed);
            assert!(!stitch(&crack(0.005), &odd).changed);
        }
    }

    #[test]
    fn two_slits_meeting_at_a_corner_zip_shut() {
        // The cube with its top face replaced by a fan round a centre point, whose last triangle
        // ends 0.006 mm short of the first corner: two slits, one along the -x edge and one to the
        // centre, meet at that corner. Top corners: 4 (0, 0), 5 (20, 0), 6 (20, 20), 7 (0, 20).
        let mut mesh = without_face(make_box([0.0; 3], [20.0; 3]), 1);
        mesh.positions.extend_from_slice(&[10.0, 10.0, 20.0]);
        mesh.positions.extend_from_slice(&[0.006, 0.0, 20.0]);
        let (centre, short) = (8, 9);
        mesh.indices
            .extend_from_slice(&[4, 5, centre, 5, 6, centre, 6, 7, centre, 7, short, centre]);
        let before = status(&mesh);
        assert_eq!((before.holes, before.border_edges), (1, 4));
        let out = stitch_checked(&mesh, &[], &TOLERANCE);
        assert_eq!(out.message, "Stitched 2 edges.");
        let s = status(&out.mesh);
        assert!(s.closed && s.oriented, "{s:?}");
        assert_eq!(cm3(s.volume), "8.000");
    }

    /// A torus damaged at random: some vertices split into a copy per triangle around them, each
    /// copy moved by up to twice the tolerance (so some fit and some do not), some triangles
    /// deleted and some turned round.
    fn damaged_torus(seed: u64) -> Mesh {
        let mut rng = crate::repair::exact::testing::Rng(seed);
        let (nu, nv) = (6 + rng.below(10) as usize, 4 + rng.below(8) as usize);
        let torus = crate::repair::exact::testing::torus(10.0, 4.0, nu, nv);
        let mut m = torus.clone();
        let split_chance = [0.05, 0.2, 0.5][rng.below(3) as usize];
        for v in 0..torus.vertex_count() as u32 {
            if !rng.chance(split_chance) {
                continue;
            }
            for slot in 0..m.indices.len() {
                if m.indices[slot] != v || rng.chance(0.3) {
                    continue;
                }
                let mut p = torus.vertex(v);
                for x in &mut p {
                    *x += ((rng.unit() * 2.0 - 1.0) * 0.02 / 3f64.sqrt()) as f32;
                }
                m.positions.extend_from_slice(&p);
                m.indices[slot] = (m.positions.len() / 3 - 1) as u32;
            }
        }
        let keep: Vec<bool> = (0..m.triangle_count()).map(|_| !rng.chance(0.03)).collect();
        let (m, _) = crate::repair::topology::keep_triangles(&m, &keep);
        // In every other mesh, some border triangles split at a point along their border side (a
        // hair off it), so the lips get vertices the other side lacks: T-junctions.
        let edges = Edges::new(&m);
        let mut out = m.clone();
        let mut split = vec![false; m.triangle_count()];
        let chance = if seed.is_multiple_of(2) { 0.3 } else { 0.0 };
        for e in 0..edges.len() as u32 {
            if edges.degree(e) != 1 || !rng.chance(chance) {
                continue;
            }
            let side = edges.sides(e)[0];
            let t = (side / 3) as usize;
            if split[t] {
                continue;
            }
            split[t] = true;
            let k = (side % 3) as usize;
            let c = [0, 1, 2].map(|i| m.indices[t * 3 + i]);
            let (p, q, y) = (c[k], c[(k + 1) % 3], c[(k + 2) % 3]);
            let f = 0.2 + 0.6 * rng.unit();
            let mut s = add(m.point(p), scale(sub(m.point(q), m.point(p)), f));
            for x in &mut s {
                *x += (rng.unit() * 2.0 - 1.0) * 0.003 / 3f64.sqrt();
            }
            let id = out.vertex_count() as u32;
            out.positions
                .extend_from_slice(&[s[0] as f32, s[1] as f32, s[2] as f32]);
            out.indices[t * 3..t * 3 + 3].copy_from_slice(&[p, id, y]);
            out.indices.extend_from_slice(&[id, q, y]);
        }
        let mut m = out;
        for t in m.indices.as_chunks_mut::<3>().0 {
            if rng.chance(0.05) {
                t.swap(1, 2);
            }
        }
        m
    }

    #[test]
    fn randomly_damaged_meshes_keep_every_promise() {
        let (mut changed, mut splits) = (0, 0);
        for seed in 0..120u64 {
            let mesh = damaged_torus(seed);
            let mut rng = crate::repair::exact::testing::Rng(seed ^ 0xABCD);
            let selection: Vec<u32> = if seed.is_multiple_of(3) {
                (0..mesh.triangle_count() as u32)
                    .filter(|_| rng.chance(0.6))
                    .collect()
            } else {
                Vec::new()
            };
            for params in [TOLERANCE, ANY_WAY] {
                let out = stitch_checked(&mesh, &selection, &params);
                changed += out.changed as usize;
                // A split triangle's pieces share its origin.
                if let Origin::Map(map) = &out.origin {
                    let mut origins = map.clone();
                    origins.sort_unstable();
                    origins.dedup();
                    splits += (origins.len() < map.len()) as usize;
                }
                // Without Ignore orientation no edge may end up with triangles facing opposite
                // ways that did not have them before.
                if !params.ignore_orientation && out.changed {
                    let inconsistent = |m: &Mesh| {
                        let edges = Edges::new(m);
                        (0..edges.len() as u32)
                            .filter(|&e| {
                                edges.degree(e) == 2
                                    && !crate::repair::topology::is_consistent(m, &edges, e)
                            })
                            .count()
                    };
                    assert!(
                        inconsistent(&out.mesh) <= inconsistent(&mesh),
                        "seed {seed}: {} -> {}",
                        inconsistent(&mesh),
                        inconsistent(&out.mesh)
                    );
                }
            }
        }
        // The damage is mostly within reach of the stitch, T-junctions included.
        assert!(changed > 150, "{changed}");
        assert!(splits > 20, "{splits}");
    }

    #[test]
    fn of_the_fixtures_only_the_crack_stitches() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repair");
        let mut stitched = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "stl") {
                continue;
            }
            let imported =
                crate::formats::import_file(&path, true, &mut crate::progress::Progress::silent())
                    .unwrap();
            let mesh = &imported.parts[0].mesh;
            for params in [TOLERANCE, ANY_WAY] {
                if stitch_checked(mesh, &[], &params).changed {
                    stitched.push(path.file_name().unwrap().to_string_lossy().into_owned());
                }
            }
        }
        assert_eq!(stitched, vec!["cube-crack.stl"; 2]);
    }

    #[test]
    fn a_cancelled_stitch_stops_and_progress_reaches_the_end() {
        let mesh = crack(0.005);
        let cancel = AtomicBool::new(true);
        let mut control = Control::new(&cancel, |_| {});
        assert_eq!(
            run(&mesh, &[], &TOLERANCE, &mut control),
            Err(RepairError::Cancelled)
        );
        let never = AtomicBool::new(false);
        let mut seen = Vec::new();
        {
            let mut control = Control::new(&never, |f| seen.push(f));
            run(&mesh, &[], &TOLERANCE, &mut control).unwrap();
        }
        assert!(seen.is_sorted() && seen.last() == Some(&1.0), "{seen:?}");
    }

    /// A closed torus of `nu` × `nv` quads cut open along every `every`-th ring: the triangles
    /// after each cut get their own copy of the ring's vertices, moved by up to `jitter` mm.
    fn cut_torus(nu: usize, nv: usize, every: usize, jitter: f32) -> Mesh {
        let mut m = crate::repair::exact::testing::torus(40.0, 15.0, nu, nv);
        let mut rng = crate::repair::exact::testing::Rng(5);
        for ring in (0..nu).step_by(every) {
            let base = m.vertex_count() as u32;
            for j in 0..nv {
                let mut p = m.vertex((ring * nv + j) as u32);
                for x in &mut p {
                    *x += ((rng.unit() * 2.0 - 1.0) * jitter as f64 / 3f64.sqrt()) as f32;
                }
                m.positions.extend_from_slice(&p);
            }
            // The strip between this ring and the next uses the copy.
            for t in ring * nv * 2..(ring + 1) * nv * 2 {
                for k in 0..3 {
                    let v = m.indices[t * 3 + k] as usize;
                    if v / nv == ring {
                        m.indices[t * 3 + k] = base + (v % nv) as u32;
                    }
                }
            }
        }
        m
    }

    #[test]
    fn a_torus_cut_into_strips_stitches_back_together() {
        let mesh = cut_torus(48, 24, 6, 0.004);
        let before = status(&mesh);
        assert_eq!((before.shells, before.holes), (8, 16));
        let out = stitch_checked(&mesh, &[], &TOLERANCE);
        let s = status(&out.mesh);
        assert!(s.closed && s.oriented, "{s:?}");
        assert_eq!((s.shells, s.triangles), (1, before.triangles));
        assert_eq!(out.message, format!("Stitched {} edges.", 8 * 24));
        let edges = Edges::new(&out.mesh);
        assert_eq!(components(&out.mesh, &edges, Link::ManifoldEdge).1, 1);
        assert!(boundary_loops(&out.mesh, &edges).is_empty());
    }

    /// A closed torus cut into patches along every `every`-th ring and meridian: each patch gets
    /// its own copies of its boundary vertices (the first patch to use a vertex keeps it), moved by
    /// up to `jitter` mm, so four lips meet at every patch corner.
    fn torus_patches(nu: usize, nv: usize, every: usize, jitter: f32) -> (Mesh, Mesh) {
        let torus = crate::repair::exact::testing::torus(40.0, 15.0, nu, nv);
        let mut m = torus.clone();
        let mut rng = crate::repair::exact::testing::Rng(9);
        let mut copies: HashMap<(usize, u32), u32> = HashMap::new();
        let mut owner: HashMap<u32, usize> = HashMap::new();
        for t in 0..torus.triangle_count() {
            let quad = t / 2;
            let patch = (quad / nv / every) * nv + (quad % nv / every);
            for k in 0..3 {
                let v = torus.indices[t * 3 + k];
                let (i, j) = (v as usize / nv, v as usize % nv);
                if !i.is_multiple_of(every) && !j.is_multiple_of(every) {
                    continue;
                }
                let first = *owner.entry(v).or_insert(patch);
                if first == patch {
                    continue;
                }
                let id = *copies.entry((patch, v)).or_insert_with(|| {
                    let mut p = torus.vertex(v);
                    for x in &mut p {
                        *x += ((rng.unit() * 2.0 - 1.0) * jitter as f64 / 3f64.sqrt()) as f32;
                    }
                    m.positions.extend_from_slice(&p);
                    (m.positions.len() / 3 - 1) as u32
                });
                m.indices[t * 3 + k] = id;
            }
        }
        (torus, m)
    }

    #[test]
    fn a_torus_cut_into_patches_stitches_back_where_four_lips_meet() {
        let (torus, patches) = torus_patches(48, 24, 6, 0.004);
        let before = status(&patches);
        assert_eq!(before.shells, 32);
        for params in [TOLERANCE, ANY_WAY] {
            let out = stitch_checked(&patches, &[], &params);
            let s = status(&out.mesh);
            assert!(s.closed && s.oriented, "{s:?}");
            assert_eq!((s.shells, s.triangles), (1, before.triangles));
            // Every copy merged into the vertex it was made from, which kept its place.
            assert_eq!(out.mesh.vertex_count(), torus.vertex_count());
            assert_eq!(s.volume, status(&torus).volume);
        }
    }

    #[test]
    fn a_jittered_triangle_soup_stitches_back_into_the_closed_shell() {
        // Every triangle of a fine torus with vertices of its own, each moved by up to 0.003 mm.
        let torus = crate::repair::exact::testing::torus(40.0, 15.0, 24, 12);
        let mut rng = crate::repair::exact::testing::Rng(3);
        let mut positions = Vec::new();
        for &v in &torus.indices {
            for x in torus.vertex(v) {
                positions.push(x + ((rng.unit() * 2.0 - 1.0) * 0.003 / 3f64.sqrt()) as f32);
            }
        }
        let soup = Mesh {
            indices: (0..torus.indices.len() as u32).collect(),
            positions,
        };
        assert_eq!(status(&soup).shells, torus.triangle_count() as u64);
        let out = stitch_checked(&soup, &[], &TOLERANCE);
        let s = status(&out.mesh);
        assert!(s.closed && s.oriented, "{s:?}");
        assert_eq!((s.shells, s.triangles), (1, torus.triangle_count() as u64));
        assert_eq!(out.mesh.vertex_count(), torus.vertex_count());
    }

    /// `cargo test --release --lib repair::stitch -- --ignored --nocapture`: a closed torus, the
    /// same torus cut into strips and into patches (about 730k triangles each), a jittered soup of
    /// 205k triangles (every vertex on the border), then the files in TRICKY3D_MODELS
    /// ("a.stl;b.stl"), each with both orientation settings and checked to stitch nothing more on
    /// a second run.
    #[test]
    #[ignore]
    fn timing_on_730k_triangles() {
        let closed = crate::repair::exact::testing::torus(40.0, 15.0, 604, 604);
        let cut = cut_torus(604, 604, 60, 0.004);
        let report = |name: &str, mesh: &Mesh, params: &StitchParams| {
            let before = status(mesh);
            // The best of three runs: other work on the machine only ever adds time.
            let mut took = f64::INFINITY;
            let mut out = None;
            for _ in 0..3 {
                let started = Instant::now();
                out = Some(stitch(mesh, params));
                took = took.min(started.elapsed().as_secs_f64());
            }
            let out = out.unwrap();
            let after = status(&out.mesh);
            println!(
                "{name} (ignore orientation {}): {} triangles, {:.0} ms: {} border edges {} -> {}, \
                 holes {} -> {}, shells {} -> {}, non-manifold {} -> {}",
                params.ignore_orientation,
                mesh.triangle_count(),
                took * 1000.0,
                out.message,
                before.border_edges,
                after.border_edges,
                before.holes,
                after.holes,
                before.shells,
                after.shells,
                before.non_manifold_edges,
                after.non_manifold_edges
            );
            out
        };
        report("closed torus", &closed, &TOLERANCE);
        let out = report("cut torus", &cut, &TOLERANCE);
        assert!(status(&out.mesh).closed);
        let (_, patches) = torus_patches(604, 604, 60, 0.004);
        let out = report("torus in patches", &patches, &TOLERANCE);
        assert!(status(&out.mesh).closed);
        // The worst case: a soup, every vertex on the border (about 200k triangles).
        let fine = crate::repair::exact::testing::torus(40.0, 15.0, 320, 320);
        let mut rng = crate::repair::exact::testing::Rng(3);
        let mut positions = Vec::new();
        for &v in &fine.indices {
            for x in fine.vertex(v) {
                positions.push(x + ((rng.unit() * 2.0 - 1.0) * 0.003 / 3f64.sqrt()) as f32);
            }
        }
        let soup = Mesh {
            indices: (0..fine.indices.len() as u32).collect(),
            positions,
        };
        let out = report("jittered soup", &soup, &TOLERANCE);
        assert!(status(&out.mesh).closed);
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
                for params in [TOLERANCE, ANY_WAY] {
                    let out = report(path, &part.mesh, &params);
                    let again = stitch(&out.mesh, &params);
                    assert!(!again.changed, "{path}: second run: {}", again.message);
                }
            }
        }
    }
}
