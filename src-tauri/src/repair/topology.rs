//! How a mesh's triangles connect: undirected edges with the triangle sides on each, border loops,
//! connected components, orientation patches, and compaction. Linear-time (counting sorts, no hash
//! map per corner), since repair runs on meshes of millions of triangles.
//!
//! A *side* is one triangle's use of an edge: side `3 * t + k` runs from corner k of triangle t to
//! corner (k + 1) % 3.

use crate::mesh::{Mesh, bounds, vertex_roots};
use crate::ops::compact;

pub const NONE: u32 = u32::MAX;

/// The undirected edges of a mesh and the sides on each. Edges are numbered in order of (lower
/// vertex, higher vertex), the order `for_each_edge` in src/mesh.rs visits them in, and stored as
/// compressed rows: the sides of edge e are `sides[start[e]..start[e + 1]]`.
pub struct Edges {
    /// Lower and higher vertex of each edge.
    ends: Vec<[u32; 2]>,
    start: Vec<u32>,
    sides: Vec<u32>,
    /// The edge of each side, NONE for a zero-length side.
    edge_of: Vec<u32>,
    /// The other side of each side on an edge with exactly two sides, else NONE.
    twin: Vec<u32>,
}

impl Edges {
    /// Counting sort of the sides by lower vertex, then a small sort per bucket by (higher vertex,
    /// side), as in `for_each_edge`: linear time and no hash map.
    pub fn new(mesh: &Mesh) -> Edges {
        let count = mesh.vertex_count();
        let triangles = mesh.indices.as_chunks::<3>().0;
        let mut bucket = vec![0u32; count + 1];
        for t in triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if a != b {
                    bucket[a.min(b) as usize + 1] += 1;
                }
            }
        }
        for v in 0..count {
            bucket[v + 1] += bucket[v];
        }
        let mut fill = bucket[..count].to_vec();
        // The higher vertex in the top half and the side in the bottom half, so sorting a bucket
        // groups it by edge with each edge's sides ascending.
        let mut keys = vec![0u64; bucket[count] as usize];
        for (n, t) in triangles.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if a != b {
                    let lo = a.min(b) as usize;
                    keys[fill[lo] as usize] = ((a.max(b) as u64) << 32) | (n * 3 + k) as u64;
                    fill[lo] += 1;
                }
            }
        }
        drop(fill);
        let mut ends = Vec::with_capacity(keys.len() / 2);
        let mut start = Vec::with_capacity(keys.len() / 2 + 1);
        let mut edge_of = vec![NONE; mesh.indices.len()];
        let mut twin = vec![NONE; mesh.indices.len()];
        for lo in 0..count {
            let range = bucket[lo] as usize..bucket[lo + 1] as usize;
            sort_small(&mut keys[range.clone()]);
            let mut higher = NONE;
            for i in range {
                let hi = (keys[i] >> 32) as u32;
                if hi != higher {
                    higher = hi;
                    start.push(i as u32);
                    ends.push([lo as u32, hi]);
                }
                edge_of[keys[i] as u32 as usize] = (ends.len() - 1) as u32;
            }
        }
        start.push(keys.len() as u32);
        let sides: Vec<u32> = keys.iter().map(|&key| key as u32).collect();
        drop(keys);
        // Twins are stored rather than looked up through the rows: the orientation walk and the
        // hole and fan walks ask for them all the time, mostly in mesh order.
        for pair in start.windows(2) {
            if pair[1] - pair[0] == 2 {
                let (a, b) = (sides[pair[0] as usize], sides[pair[0] as usize + 1]);
                twin[a as usize] = b;
                twin[b as usize] = a;
            }
        }
        Edges {
            ends,
            start,
            sides,
            edge_of,
            twin,
        }
    }

    /// Number of undirected edges (zero-length sides of repeated-index triangles are not edges).
    pub fn len(&self) -> usize {
        self.ends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The lower and higher vertex of edge `e`.
    pub fn ends(&self, e: u32) -> [u32; 2] {
        self.ends[e as usize]
    }

    /// The sides on edge `e`, in ascending side order.
    pub fn sides(&self, e: u32) -> &[u32] {
        let e = e as usize;
        &self.sides[self.start[e] as usize..self.start[e + 1] as usize]
    }

    /// The edge of `side`, or NONE for a zero-length side.
    pub fn edge_of(&self, side: u32) -> u32 {
        self.edge_of[side as usize]
    }

    /// Number of sides on edge `e`: 1 border, 2 manifold, 3 or more non-manifold.
    pub fn degree(&self, e: u32) -> usize {
        self.sides(e).len()
    }

    /// The other side on `side`'s edge when that edge has exactly two sides, else NONE.
    pub fn twin(&self, side: u32) -> u32 {
        self.twin[side as usize]
    }
}

/// A bucket holds one vertex's few sides, so insertion sort; big fans use the library sort.
fn sort_small(v: &mut [u64]) {
    if v.len() > 16 {
        v.sort_unstable();
        return;
    }
    for i in 1..v.len() {
        let key = v[i];
        let mut j = i;
        while j > 0 && v[j - 1] > key {
            v[j] = v[j - 1];
            j -= 1;
        }
        v[j] = key;
    }
}

/// The vertex a side starts at and the one it ends at.
#[inline]
pub fn side_ends(mesh: &Mesh, side: u32) -> (u32, u32) {
    let t = side as usize / 3;
    let k = side as usize % 3;
    (mesh.indices[t * 3 + k], mesh.indices[t * 3 + (k + 1) % 3])
}

/// True when edge `e` has exactly two sides and they run opposite ways (consistent orientation).
pub fn is_consistent(mesh: &Mesh, edges: &Edges, e: u32) -> bool {
    // Both sides join the same two vertices, so they run opposite ways when they start apart.
    match *edges.sides(e) {
        [a, b] => side_ends(mesh, a).0 != side_ends(mesh, b).0,
        _ => false,
    }
}

/// One hole's border: its border sides in walking order and the vertex the walk leaves each from.
/// The walk follows each side's own direction, so a patch that fills the hole uses every edge the
/// other way (patch triangle corners run `vertices` backwards). Only where the triangles around a
/// hole disagree in orientation does a side run against the walk; the walk then goes the way most
/// of its sides run.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundaryLoop {
    pub sides: Vec<u32>,
    pub vertices: Vec<u32>,
}

/// One step of a border walk: the side, the vertex the walk leaves from, and whether the walk runs
/// the side's own way.
#[derive(Clone, Copy)]
struct Step {
    side: u32,
    from: u32,
    forward: bool,
}

/// Every hole's border loop, each a simple cycle (no vertex twice).
///
/// The walk follows the sides' own direction. Where a vertex has several border sides leaving it
/// (holes touching at a corner), the side at the far end of the fan of triangles the walk arrived
/// through borders the other hole, so the walk takes a side of another fan (with two fans, the
/// one that goes on round the same hole). A walk that still comes back to a vertex is split there
/// into simple loops. Where orientation is inconsistent and no side leaves a vertex, the walk goes
/// on along a side that ends there. Border sides that cannot close into a loop, because they end
/// at a non-manifold edge, belong to no loop.
pub fn boundary_loops(mesh: &Mesh, edges: &Edges) -> Vec<BoundaryLoop> {
    let mut border: Vec<u32> = (0..edges.len() as u32)
        .filter(|&e| edges.degree(e) == 1)
        .map(|e| edges.sides(e)[0])
        .collect();
    if border.is_empty() {
        return Vec::new();
    }
    border.sort_unstable();
    let count = mesh.vertex_count();
    // The border sides leaving and entering each vertex, as linked lists of positions in `border`
    // in ascending order. A used side is unlinked when a list walk meets it.
    let mut out_head = vec![NONE; count];
    let mut out_next = vec![NONE; border.len()];
    let mut in_head = vec![NONE; count];
    let mut in_next = vec![NONE; border.len()];
    for (i, &side) in border.iter().enumerate().rev() {
        let (a, b) = side_ends(mesh, side);
        out_next[i] = out_head[a as usize];
        out_head[a as usize] = i as u32;
        in_next[i] = in_head[b as usize];
        in_head[b as usize] = i as u32;
    }
    let mut used = vec![false; border.len()];
    // While a walk is split: the position on `stack` of the step leaving each vertex.
    let mut on_stack = vec![NONE; count];
    let mut walk: Vec<Step> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut loops = Vec::new();
    for first in 0..border.len() {
        if used[first] {
            continue;
        }
        used[first] = true;
        let (start, mut at) = side_ends(mesh, border[first]);
        walk.clear();
        walk.push(Step {
            side: border[first],
            from: start,
            forward: true,
        });
        while at != start {
            let (next, forward) = match unused_two(&mut out_head, &mut out_next, &used, at) {
                (NONE, _) => match unused_two(&mut in_head, &mut in_next, &used, at) {
                    (NONE, _) => break,
                    (i, _) => (i, false),
                },
                (i, NONE) => (i, true),
                (i, j) => {
                    let previous = walk[walk.len() - 1].side;
                    if fan_end(mesh, edges, previous, at) == border[i as usize] {
                        (j, true)
                    } else {
                        (i, true)
                    }
                }
            };
            used[next as usize] = true;
            let side = border[next as usize];
            walk.push(Step {
                side,
                from: at,
                forward,
            });
            let (a, b) = side_ends(mesh, side);
            at = if forward { b } else { a };
        }
        // A step leaving a vertex that is already on the stack closes the steps taken since that
        // vertex into a simple loop.
        stack.clear();
        for (n, step) in walk.iter().enumerate() {
            let v = step.from as usize;
            if on_stack[v] != NONE {
                let p = on_stack[v] as usize;
                loops.push(loop_of(&walk, &stack[p..]));
                for &m in &stack[p..] {
                    on_stack[walk[m].from as usize] = NONE;
                }
                stack.truncate(p);
            }
            on_stack[v] = stack.len() as u32;
            stack.push(n);
        }
        if at == start {
            loops.push(loop_of(&walk, &stack));
        }
        for &m in &stack {
            on_stack[walk[m].from as usize] = NONE;
        }
    }
    loops
}

/// The first two unused entries of vertex `v`'s list (NONE where there are fewer), unlinking the
/// used entries met on the way, so each entry is stepped over at most once.
fn unused_two(head: &mut [u32], next: &mut [u32], used: &[bool], v: u32) -> (u32, u32) {
    let mut first = head[v as usize];
    while first != NONE && used[first as usize] {
        first = next[first as usize];
    }
    head[v as usize] = first;
    if first == NONE {
        return (NONE, NONE);
    }
    let mut second = next[first as usize];
    while second != NONE && used[second as usize] {
        second = next[second as usize];
    }
    next[first as usize] = second;
    (first, second)
}

/// The border side at the far end of the fan round vertex `at` that holds `side` (a side touching
/// `at`): turn round `at` from triangle to triangle across edges with two sides until an edge
/// without. NONE when that edge is not a border edge.
fn fan_end(mesh: &Mesh, edges: &Edges, side: u32, at: u32) -> u32 {
    let mut side = side;
    // The triangles round a vertex joined through two-sided edges form a path, so each turn
    // reaches a new one; the bound only guards against a malformed mesh.
    for _ in 0..mesh.triangle_count() {
        // The other side of this side's triangle at `at`.
        let turn = if side_ends(mesh, side).0 == at { 2 } else { 1 };
        let other = side / 3 * 3 + (side % 3 + turn) % 3;
        let e = edges.edge_of(other);
        if e == NONE {
            return NONE;
        }
        match edges.degree(e) {
            1 => return other,
            2 => side = edges.twin(other),
            _ => return NONE,
        }
    }
    NONE
}

/// The loop of the walk steps `steps`, turned round when most of its sides run against it.
fn loop_of(walk: &[Step], steps: &[usize]) -> BoundaryLoop {
    let mut sides: Vec<u32> = steps.iter().map(|&n| walk[n].side).collect();
    let mut vertices: Vec<u32> = steps.iter().map(|&n| walk[n].from).collect();
    let backward = steps.iter().filter(|&&n| !walk[n].forward).count();
    if backward * 2 > steps.len() {
        sides.reverse();
        vertices[1..].reverse();
    }
    BoundaryLoop { sides, vertices }
}

/// What joins two triangles into one component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// A shared vertex.
    Vertex,
    /// A shared edge.
    Edge,
    /// An edge with exactly two sides.
    ManifoldEdge,
    /// An edge with exactly two sides running opposite ways: shells as the reference tool counts
    /// them (and the info panel, the Shells tab and Split shells into parts), apart wherever an
    /// edge is open or non-manifold or its two triangles disagree about which way they face.
    Shell,
}

/// The component of each triangle (numbered in order of first triangle) and the count.
pub fn components(mesh: &Mesh, edges: &Edges, link: Link) -> (Vec<u32>, usize) {
    let count = mesh.triangle_count();
    let mut component = Vec::with_capacity(count);
    let mut next = 0u32;
    if link == Link::Vertex {
        let roots = vertex_roots(mesh);
        let mut id = vec![NONE; roots.len()];
        for t in mesh.indices.as_chunks::<3>().0 {
            let slot = &mut id[roots[t[0] as usize] as usize];
            if *slot == NONE {
                *slot = next;
                next += 1;
            }
            component.push(*slot);
        }
        return (component, next as usize);
    }
    // Union-find over triangles (path halving); a root is the lowest triangle of its component.
    let mut parent: Vec<u32> = (0..count as u32).collect();
    for e in 0..edges.len() as u32 {
        let sides = edges.sides(e);
        let joins = match link {
            Link::Vertex | Link::Edge => sides.len() >= 2,
            Link::ManifoldEdge => sides.len() == 2,
            Link::Shell => is_consistent(mesh, edges, e),
        };
        if !joins {
            continue;
        }
        for &s in &sides[1..] {
            let (a, b) = (root(&mut parent, sides[0] / 3), root(&mut parent, s / 3));
            if a != b {
                parent[a.max(b) as usize] = a.min(b);
            }
        }
    }
    // The root comes before every other triangle of its component, so its number is known.
    for t in 0..count as u32 {
        let r = root(&mut parent, t);
        if r == t {
            component.push(next);
            next += 1;
        } else {
            component.push(component[r as usize]);
        }
    }
    (component, next as usize)
}

fn root(parent: &mut [u32], mut v: u32) -> u32 {
    while parent[v as usize] != v {
        let up = parent[parent[v as usize] as usize];
        parent[v as usize] = up;
        v = up;
    }
    v
}

/// Orientation patches: breadth-first over edges with exactly two sides, a neighbour whose side
/// runs the same way as the current triangle's must be turned to agree.
pub struct Patches {
    pub patch_of: Vec<u32>,
    pub count: usize,
    /// Per triangle: turn it to agree with its patch's first triangle.
    pub flip: Vec<bool>,
    /// Edges with two sides whose triangles still disagree after the walk (a non-orientable patch,
    /// such as a Möbius strip).
    pub conflicts: usize,
    /// Per patch: every edge of the patch has exactly two sides, both in the patch.
    pub closed: Vec<bool>,
}

/// Patches are numbered in order of first triangle, which starts each walk, so a patch's first
/// triangle is its lowest and never flipped. Zero-length sides are not edges: they neither join
/// nor open a patch.
///
/// Each triangle's neighbours are visited in edge order, which turning triangles does not change
/// (it only renumbers their sides), so the walk is the same before and after Fix flipped
/// triangles: on a patch that cannot agree everywhere, a second run finds the same conflict edges
/// and nothing more to turn.
pub fn orientation_patches(mesh: &Mesh, edges: &Edges) -> Patches {
    let triangles = mesh.triangle_count();
    let mut patch_of = vec![NONE; triangles];
    let mut flip = vec![false; triangles];
    let mut closed = Vec::new();
    let mut queue: Vec<u32> = Vec::new();
    for first in 0..triangles {
        if patch_of[first] != NONE {
            continue;
        }
        let patch = closed.len() as u32;
        let mut shut = true;
        patch_of[first] = patch;
        queue.clear();
        queue.push(first as u32);
        let mut head = 0;
        while head < queue.len() {
            let t = queue[head];
            head += 1;
            let mut sides = [t * 3, t * 3 + 1, t * 3 + 2].map(|s| (edges.edge_of(s), s));
            sides.sort_unstable();
            for (e, side) in sides {
                if e == NONE {
                    break;
                }
                let other = edges.twin(side);
                if other == NONE {
                    shut = false;
                    continue;
                }
                let u = (other / 3) as usize;
                if patch_of[u] == NONE {
                    // Two sides running the same way (starting at the same vertex): one of the
                    // triangles must turn.
                    let same = mesh.indices[side as usize] == mesh.indices[other as usize];
                    flip[u] = flip[t as usize] ^ same;
                    patch_of[u] = patch;
                    queue.push(u as u32);
                }
            }
        }
        closed.push(shut);
    }
    let mut conflicts = 0;
    for e in 0..edges.len() as u32 {
        if let [s, r] = *edges.sides(e) {
            let (t, u) = ((s / 3) as usize, (r / 3) as usize);
            let same = mesh.indices[s as usize] == mesh.indices[r as usize];
            if t != u && same == (flip[t] == flip[u]) {
                conflicts += 1;
            }
        }
    }
    Patches {
        patch_of,
        count: closed.len(),
        flip,
        conflicts,
        closed,
    }
}

/// The mesh with only the triangles where `keep` is true, in their order, and unused vertices
/// dropped (vertex order kept). Also the origin map: input triangle of each output triangle.
pub fn keep_triangles(mesh: &Mesh, keep: &[bool]) -> (Mesh, Vec<u32>) {
    debug_assert_eq!(keep.len(), mesh.triangle_count());
    let kept = keep.iter().filter(|&&k| k).count();
    let mut indices = Vec::with_capacity(kept * 3);
    let mut origin = Vec::with_capacity(kept);
    for (t, (corners, _)) in mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .zip(keep)
        .enumerate()
        .filter(|(_, (_, k))| **k)
    {
        indices.extend_from_slice(corners);
        origin.push(t as u32);
    }
    (compact(&mesh.positions, indices), origin)
}

/// Length of the bounding-box diagonal of the vertices triangles use (0 for an empty mesh).
pub fn bbox_diagonal(mesh: &Mesh) -> f64 {
    let Some([lo, hi]) = bounds(mesh) else {
        return 0.0;
    };
    (0..3)
        .map(|k| (hi[k] as f64 - lo[k] as f64).powi(2))
        .sum::<f64>()
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{make_box, stats};
    use crate::ops::concat;

    fn cube() -> Mesh {
        make_box([0.0; 3], [20.0; 3])
    }

    fn mesh(positions: &[f32], indices: &[u32]) -> Mesh {
        Mesh {
            positions: positions.to_vec(),
            indices: indices.to_vec(),
        }
    }

    /// The 20 mm cube without some of its faces: 0 −z, 1 +z, 2 −y, 3 +y, 4 +x, 5 −x (make_box
    /// order, two triangles each).
    fn cube_without(faces: &[usize]) -> Mesh {
        let mut m = cube();
        m.indices = m
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .enumerate()
            .filter(|(t, _)| !faces.contains(&(t / 2)))
            .flat_map(|(_, c)| *c)
            .collect();
        m
    }

    fn flip(mesh: &mut Mesh, t: usize) {
        mesh.indices.swap(t * 3 + 1, t * 3 + 2);
    }

    /// A regular octahedron with outward triangles; the first four meet at the top vertex 4.
    fn octahedron() -> Mesh {
        mesh(
            &[
                1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0, 0.0,
                0.0, -1.0,
            ],
            &[
                0, 2, 4, 2, 1, 4, 1, 3, 4, 3, 0, 4, // top
                2, 0, 5, 1, 2, 5, 3, 1, 5, 0, 3, 5, // bottom
            ],
        )
    }

    /// A hexagonal bipyramid: ring vertices 0..6, top apex 6, bottom apex 7. Triangles 0..6 meet
    /// at the top apex.
    fn bipyramid() -> Mesh {
        let mut positions = Vec::new();
        for k in 0..6 {
            let a = k as f32 * std::f32::consts::PI / 3.0;
            positions.extend_from_slice(&[a.cos(), a.sin(), 0.0]);
        }
        positions.extend_from_slice(&[0.0, 0.0, 1.0, 0.0, 0.0, -1.0]);
        let mut indices = Vec::new();
        for k in 0..6 {
            indices.extend_from_slice(&[k, (k + 1) % 6, 6]);
        }
        for k in 0..6 {
            indices.extend_from_slice(&[(k + 1) % 6, k, 7]);
        }
        mesh(&positions, &indices)
    }

    /// A Möbius strip of `n` quads (two triangles each): the last quad joins the first with top
    /// and bottom swapped.
    fn mobius(n: u32) -> Mesh {
        let mut positions = Vec::new();
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let (c, s) = ((a / 2.0).cos(), (a / 2.0).sin());
            for w in [1.0f32, -1.0] {
                let r = 4.0 + w * c;
                positions.extend_from_slice(&[r * a.cos(), r * a.sin(), w * s]);
            }
        }
        let mut indices = Vec::new();
        for i in 0..n {
            let (top, bottom) = (2 * i, 2 * i + 1);
            let (next_top, next_bottom) = if i + 1 < n {
                (2 * i + 2, 2 * i + 3)
            } else {
                (1, 0)
            };
            indices.extend_from_slice(&[top, bottom, next_bottom, top, next_bottom, next_top]);
        }
        mesh(&positions, &indices)
    }

    /// A Möbius strip two quads wide (rows top, middle, bottom), so the triangles' adjacency
    /// branches instead of forming a single ring.
    fn wide_mobius(n: u32) -> Mesh {
        let mut positions = Vec::new();
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let (c, s) = ((a / 2.0).cos(), (a / 2.0).sin());
            for w in [1.0f32, 0.0, -1.0] {
                let r = 4.0 + w * c;
                positions.extend_from_slice(&[r * a.cos(), r * a.sin(), w * s]);
            }
        }
        let mut indices = Vec::new();
        for i in 0..n {
            let next = |r: u32| if i + 1 < n { 3 * (i + 1) + r } else { 2 - r };
            for r in 0..2 {
                let (a, b) = (3 * i + r, 3 * i + r + 1);
                indices.extend_from_slice(&[a, b, next(r + 1), a, next(r + 1), next(r)]);
            }
        }
        mesh(&positions, &indices)
    }

    /// Checks what every loop promises: each side a border side in at most one loop, joined end
    /// to end from `vertices[i]` to `vertices[i + 1]` (either way round), no vertex twice.
    fn check_loops(mesh: &Mesh, edges: &Edges, loops: &[BoundaryLoop]) {
        let mut seen = vec![false; mesh.indices.len()];
        for l in loops {
            let n = l.sides.len();
            assert!(n >= 3 && l.vertices.len() == n, "{l:?}");
            let mut distinct = l.vertices.clone();
            distinct.sort_unstable();
            distinct.dedup();
            assert_eq!(distinct.len(), n, "a vertex twice: {l:?}");
            for (i, &s) in l.sides.iter().enumerate() {
                assert_eq!(
                    edges.degree(edges.edge_of(s)),
                    1,
                    "not a border side: {l:?}"
                );
                assert!(!seen[s as usize], "side {s} in two loops");
                seen[s as usize] = true;
                let (a, b) = side_ends(mesh, s);
                let (from, to) = (l.vertices[i], l.vertices[(i + 1) % n]);
                assert!((a, b) == (from, to) || (a, b) == (to, from), "{l:?}");
            }
        }
    }

    /// How many of a loop's sides run the walk's way.
    fn forward(mesh: &Mesh, l: &BoundaryLoop) -> usize {
        l.sides
            .iter()
            .zip(&l.vertices)
            .filter(|&(&s, &v)| side_ends(mesh, s).0 == v)
            .count()
    }

    fn loops_of(mesh: &Mesh) -> Vec<BoundaryLoop> {
        let edges = Edges::new(mesh);
        let loops = boundary_loops(mesh, &edges);
        check_loops(mesh, &edges, &loops);
        loops
    }

    fn lengths(loops: &[BoundaryLoop]) -> Vec<usize> {
        let mut out: Vec<usize> = loops.iter().map(|l| l.sides.len()).collect();
        out.sort_unstable();
        out
    }

    #[test]
    fn a_cube_has_eighteen_consistent_two_sided_edges() {
        let m = cube();
        let edges = Edges::new(&m);
        assert_eq!(edges.len(), 18);
        assert!(!edges.is_empty());
        let mut previous = [0u32, 0];
        for e in 0..edges.len() as u32 {
            let [lo, hi] = edges.ends(e);
            assert!(lo < hi && (e == 0 || [lo, hi] > previous));
            previous = [lo, hi];
            assert_eq!(edges.degree(e), 2);
            assert!(edges.sides(e).is_sorted());
            assert!(is_consistent(&m, &edges, e));
            for &s in edges.sides(e) {
                assert_eq!(edges.edge_of(s), e);
                let (a, b) = side_ends(&m, s);
                assert_eq!([a.min(b), a.max(b)], [lo, hi]);
                let twin = edges.twin(s);
                assert_ne!(twin, s);
                assert_eq!(edges.twin(twin), s);
            }
        }
        for s in 0..36 {
            assert_ne!(edges.edge_of(s), NONE);
        }
    }

    #[test]
    fn zero_length_sides_are_not_edges() {
        // The repeated-index triangle uses edge 0-1 twice (0→1 and 1→0), the other triangle once.
        let m = mesh(
            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            &[0, 0, 1, 0, 1, 2],
        );
        let edges = Edges::new(&m);
        assert_eq!(edges.len(), 3);
        assert_eq!(edges.edge_of(0), NONE);
        assert_eq!(edges.twin(0), NONE);
        let e = edges.edge_of(1);
        assert_eq!(edges.ends(e), [0, 1]);
        assert_eq!(edges.sides(e), &[1, 2, 3]);
        assert_eq!(edges.twin(1), NONE);
        assert!(!is_consistent(&m, &edges, e));
        assert_eq!(edges.degree(edges.edge_of(4)), 1);
        // Nothing at all.
        let empty = Edges::new(&Mesh::default());
        assert!(empty.is_empty());
    }

    #[test]
    fn a_turned_triangle_makes_its_three_edges_inconsistent() {
        let mut m = cube();
        flip(&mut m, 5);
        let edges = Edges::new(&m);
        let bad: Vec<u32> = (0..18).filter(|&e| !is_consistent(&m, &edges, e)).collect();
        assert_eq!(bad.len(), 3);
        for e in bad {
            assert!(edges.sides(e).iter().any(|&s| s / 3 == 5));
        }
    }

    #[test]
    fn a_single_triangle_is_one_loop_of_three() {
        let m = mesh(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0], &[0, 1, 2]);
        let loops = loops_of(&m);
        assert_eq!(
            loops,
            vec![BoundaryLoop {
                sides: vec![0, 1, 2],
                vertices: vec![0, 1, 2],
            }]
        );
    }

    #[test]
    fn closed_and_empty_meshes_have_no_loops() {
        assert!(loops_of(&cube()).is_empty());
        assert!(loops_of(&Mesh::default()).is_empty());
    }

    #[test]
    fn a_cube_missing_one_face_has_one_loop_of_four_in_side_order() {
        let m = cube_without(&[1]);
        let loops = loops_of(&m);
        assert_eq!(lengths(&loops), vec![4]);
        let l = &loops[0];
        // Every side runs the walk's way: side i ends where side i + 1 starts.
        assert_eq!(forward(&m, l), 4);
        for i in 0..4 {
            assert_eq!(side_ends(&m, l.sides[i]).1, l.vertices[(i + 1) % 4]);
        }
        // The open face's corners are 4..8 (z = 20).
        let mut corners = l.vertices.clone();
        corners.sort_unstable();
        assert_eq!(corners, vec![4, 5, 6, 7]);
        // A patch running the vertices backwards closes the cube consistently and outward (the
        // top face adds a third of the volume, so a patch facing in would leave 2666.7).
        assert!((stats(&m).volume - 16000.0 / 3.0).abs() < 1e-9);
        let mut closed = m.clone();
        let v = &l.vertices;
        closed
            .indices
            .extend_from_slice(&[v[0], v[3], v[2], v[0], v[2], v[1]]);
        let edges = Edges::new(&closed);
        assert!((0..edges.len() as u32).all(|e| is_consistent(&closed, &edges, e)));
        assert_eq!(stats(&closed).volume, 8000.0);
    }

    #[test]
    fn a_cube_missing_two_adjacent_faces_has_one_loop_of_six() {
        let loops = loops_of(&cube_without(&[0, 2]));
        assert_eq!(lengths(&loops), vec![6]);
    }

    #[test]
    fn a_cube_missing_two_opposite_faces_has_two_loops_of_four() {
        let m = cube_without(&[0, 1]);
        let loops = loops_of(&m);
        assert_eq!(lengths(&loops), vec![4, 4]);
        assert!(loops.iter().all(|l| forward(&m, l) == 4));
    }

    #[test]
    fn two_holes_touching_at_a_vertex_are_two_loops() {
        // The octahedron without two top triangles that share only the top vertex.
        let mut m = octahedron();
        m.indices.drain(6..9);
        m.indices.drain(0..3);
        let loops = loops_of(&m);
        assert_eq!(lengths(&loops), vec![3, 3]);
        assert!(loops.iter().all(|l| forward(&m, l) == 3));
        // Each loop is one removed triangle's corners.
        let mut corners: Vec<Vec<u32>> = loops
            .iter()
            .map(|l| {
                let mut v = l.vertices.clone();
                v.sort_unstable();
                v
            })
            .collect();
        corners.sort();
        assert_eq!(corners, vec![vec![0, 2, 4], vec![1, 3, 4]]);
        // Whichever side the walks start from.
        for rotate in 1..m.triangle_count() {
            let mut turned = m.clone();
            turned.indices.rotate_left(rotate * 3);
            assert_eq!(lengths(&loops_of(&turned)), vec![3, 3]);
        }
    }

    #[test]
    fn three_holes_touching_at_a_vertex_are_three_loops() {
        let mut m = bipyramid();
        let keep: Vec<bool> = (0..12usize)
            .map(|t| !(t < 6 && t.is_multiple_of(2)))
            .collect();
        m = keep_triangles(&m, &keep).0;
        let loops = loops_of(&m);
        assert_eq!(lengths(&loops), vec![3, 3, 3]);
        for rotate in 1..m.triangle_count() {
            let mut turned = m.clone();
            turned.indices.rotate_left(rotate * 3);
            assert_eq!(lengths(&loops_of(&turned)), vec![3, 3, 3]);
        }
    }

    #[test]
    fn two_holes_touching_at_two_vertices_stay_apart() {
        // A square frame (inner corners 0..8 counter-clockwise from (1, 1), outer corners 8..16)
        // with an island triangle (7, 3, 16) pinned between its inner sides at b = 7 and c = 3.
        // Above the island is one hole (6 sides), below it another (5 sides), and the frame's
        // outer rim is a third loop (8). Pairing each arriving side with the far end of its own
        // fan would instead give the frame's inner ring (8) and the island's outline (3), both
        // simple but each mixing the two holes.
        let inner = [
            [1.0, 1.0],
            [2.0, 1.0],
            [3.0, 1.0],
            [3.0, 2.0],
            [3.0, 3.0],
            [2.0, 3.0],
            [1.0, 3.0],
            [1.0, 2.0],
        ];
        let mut positions = Vec::new();
        for [x, y] in inner {
            positions.extend_from_slice(&[x, y, 0.0]);
        }
        for [x, y] in inner {
            positions.extend_from_slice(&[2.0 * x - 2.0, 2.0 * y - 2.0, 0.0]);
        }
        positions.extend_from_slice(&[2.0, 2.5, 0.0]);
        let mut indices = Vec::new();
        for i in 0..8 {
            let j = (i + 1) % 8;
            indices.extend_from_slice(&[i, 8 + i, 8 + j, i, 8 + j, j]);
        }
        indices.extend_from_slice(&[7, 3, 16]);
        let m = mesh(&positions, &indices);
        for rotate in 0..m.triangle_count() {
            let mut turned = m.clone();
            turned.indices.rotate_left(rotate * 3);
            let loops = loops_of(&turned);
            assert_eq!(lengths(&loops), vec![5, 6, 8], "rotated by {rotate}");
            let edges = Edges::new(&turned);
            assert_eq!(
                loops.iter().map(|l| l.sides.len()).sum::<usize>(),
                (0..edges.len() as u32)
                    .filter(|&e| edges.degree(e) == 1)
                    .count()
            );
            for l in &loops {
                assert_eq!(forward(&turned, l), l.sides.len());
                // The big hole runs along the island's two top edges, the small one along its
                // bottom edge 7→3; the rim has only outer corners.
                assert_eq!(l.vertices.contains(&16), l.sides.len() == 6);
                assert_eq!(l.vertices.iter().all(|&v| v >= 8), l.sides.len() == 8);
                if l.sides.len() == 5 {
                    assert!(l.sides.iter().any(|&s| side_ends(&turned, s) == (7, 3)));
                }
            }
        }
    }

    #[test]
    fn two_triangles_sharing_a_corner_are_two_loops() {
        let m = mesh(
            &[
                0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, -1.0, 0.0, 0.0, -1.0, -1.0, 0.0,
            ],
            &[0, 1, 2, 0, 3, 4],
        );
        assert_eq!(lengths(&loops_of(&m)), vec![3, 3]);
    }

    #[test]
    fn a_hole_beside_a_turned_triangle_is_still_one_loop() {
        // The -y triangle on the open bottom edge 0-1 is turned, so that border side runs the
        // other way; the loop still goes round once, the way the other three run.
        let mut m = cube_without(&[0]);
        let t = m
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .position(|c| *c == [0, 1, 5])
            .unwrap();
        flip(&mut m, t);
        let loops = loops_of(&m);
        assert_eq!(lengths(&loops), vec![4]);
        assert_eq!(forward(&m, &loops[0]), 3);
        // The same when the walk has to start on the odd side.
        let mut turned = m.clone();
        turned.indices.rotate_left(t * 3);
        let loops = loops_of(&turned);
        assert_eq!(lengths(&loops), vec![4]);
        assert_eq!(forward(&turned, &loops[0]), 3);
    }

    #[test]
    fn a_mobius_strip_has_one_border_loop() {
        let m = mobius(12);
        let loops = loops_of(&m);
        assert_eq!(lengths(&loops), vec![24]);
        let wide = wide_mobius(12);
        assert_eq!(lengths(&loops_of(&wide)), vec![24]);
        let p = orientation_patches(&wide, &Edges::new(&wide));
        assert_eq!((p.count, p.closed.clone()), (1, vec![false]));
        assert!(p.conflicts >= 1);
    }

    #[test]
    fn border_sides_ending_at_a_non_manifold_edge_may_close_no_loop() {
        // Three fins on edge 0-1: two of them make a loop round their border edges, and the third
        // fin's two border sides end at the non-manifold edge with nothing to close them.
        let m = mesh(
            &[
                0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 0.0,
            ],
            &[0, 1, 2, 0, 1, 3, 0, 1, 4],
        );
        let loops = loops_of(&m);
        assert_eq!(lengths(&loops), vec![4]);
    }

    #[test]
    fn many_disjoint_triangles_are_one_loop_each() {
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for i in 0..100u32 {
            let x = i as f32 * 3.0;
            positions.extend_from_slice(&[x, 0.0, 0.0, x + 1.0, 0.0, 0.0, x, 1.0, 0.0]);
            indices.extend_from_slice(&[i * 3, i * 3 + 1, i * 3 + 2]);
        }
        let loops = loops_of(&mesh(&positions, &indices));
        assert_eq!(lengths(&loops), vec![3; 100]);
    }

    #[test]
    fn components_by_vertex_edge_manifold_edge_and_shell() {
        // Two cubes with their own vertices, their triangles interleaved: two components in every
        // sense, numbered in order of first triangle.
        let a = cube();
        let b = make_box([30.0; 3], [40.0; 3]);
        let mut m = Mesh {
            positions: [a.positions.clone(), b.positions.clone()].concat(),
            indices: Vec::new(),
        };
        for k in 0..12 {
            m.indices
                .extend(b.indices[k * 3..k * 3 + 3].iter().map(|i| i + 8));
            m.indices.extend_from_slice(&a.indices[k * 3..k * 3 + 3]);
        }
        let edges = Edges::new(&m);
        for link in [Link::Vertex, Link::Edge, Link::ManifoldEdge, Link::Shell] {
            let (component, count) = components(&m, &edges, link);
            assert_eq!(count, 2);
            assert!(
                component
                    .iter()
                    .enumerate()
                    .all(|(t, &c)| c == (t % 2) as u32)
            );
        }

        // A second cube sharing only the corner vertex (20, 20, 20): one shell, two by edges.
        let mut glued = concat(&[&a, &make_box([20.0; 3], [40.0; 3])]).unwrap();
        for i in &mut glued.indices {
            if *i == 8 {
                *i = 6;
            }
        }
        let edges = Edges::new(&glued);
        assert_eq!(components(&glued, &edges, Link::Vertex).1, 1);
        assert_eq!(components(&glued, &edges, Link::Edge).1, 2);
        assert_eq!(components(&glued, &edges, Link::ManifoldEdge).1, 2);
        assert_eq!(components(&glued, &edges, Link::Shell).1, 2);

        // Three fins on one edge: one by vertex and by edge, three by manifold edge.
        let fins = mesh(
            &[
                0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 0.0,
            ],
            &[0, 1, 2, 0, 1, 3, 0, 1, 4],
        );
        let edges = Edges::new(&fins);
        assert_eq!(components(&fins, &edges, Link::Vertex), (vec![0, 0, 0], 1));
        assert_eq!(components(&fins, &edges, Link::Edge), (vec![0, 0, 0], 1));
        assert_eq!(
            components(&fins, &edges, Link::ManifoldEdge),
            (vec![0, 1, 2], 3)
        );
        assert_eq!(components(&fins, &edges, Link::Shell), (vec![0, 1, 2], 3));

        // A cube with one triangle turned over: one shell by manifold edge, two by shell (the
        // turned triangle disagrees with all three neighbours).
        let mut turned = cube();
        turned.indices.swap(4, 5);
        let edges = Edges::new(&turned);
        assert_eq!(components(&turned, &edges, Link::ManifoldEdge).1, 1);
        let (shell_of, count) = components(&turned, &edges, Link::Shell);
        assert_eq!(count, 2);
        assert_eq!(shell_of.iter().filter(|&&s| s == shell_of[1]).count(), 1);
        // No triangles, no components.
        let empty = Mesh::default();
        assert_eq!(
            components(&empty, &Edges::new(&empty), Link::Edge),
            (vec![], 0)
        );
    }

    #[test]
    fn a_triangle_alone_on_a_vertex_is_its_own_edge_component() {
        // Triangle 2 touches the strip of triangles 0 and 1 only at vertex 2: one component by
        // vertex, two by edge.
        let m = mesh(
            &[
                0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, -1.0, 2.0, 0.0, -1.0,
                1.0, 0.0,
            ],
            &[0, 1, 2, 1, 3, 2, 2, 4, 5],
        );
        let edges = Edges::new(&m);
        assert_eq!(components(&m, &edges, Link::Vertex), (vec![0, 0, 0], 1));
        assert_eq!(components(&m, &edges, Link::Edge), (vec![0, 0, 1], 2));
    }

    #[test]
    fn a_cube_is_one_closed_patch() {
        let m = cube();
        let p = orientation_patches(&m, &Edges::new(&m));
        assert_eq!((p.count, p.conflicts), (1, 0));
        assert_eq!(p.closed, vec![true]);
        assert!(p.patch_of.iter().all(|&x| x == 0));
        assert!(p.flip.iter().all(|&f| !f));
    }

    #[test]
    fn turned_triangles_are_marked_to_flip() {
        let mut m = cube();
        for t in [3, 7, 10] {
            flip(&mut m, t);
        }
        let p = orientation_patches(&m, &Edges::new(&m));
        assert_eq!((p.count, p.conflicts, p.closed.clone()), (1, 0, vec![true]));
        let flipped: Vec<usize> = (0..12).filter(|&t| p.flip[t]).collect();
        assert_eq!(flipped, vec![3, 7, 10]);
        // When the first triangle is the odd one, the others are the ones to turn.
        let mut m = cube();
        flip(&mut m, 0);
        let p = orientation_patches(&m, &Edges::new(&m));
        assert_eq!(p.flip.iter().filter(|&&f| f).count(), 11);
        assert!(!p.flip[0]);
        // An inside-out cube is consistent within itself.
        let mut m = cube();
        for t in 0..12 {
            flip(&mut m, t);
        }
        let p = orientation_patches(&m, &Edges::new(&m));
        assert!(p.flip.iter().all(|&f| !f) && p.conflicts == 0);
    }

    #[test]
    fn open_shells_separate_bodies_and_fins_make_separate_patches() {
        let m = cube_without(&[3]);
        let p = orientation_patches(&m, &Edges::new(&m));
        assert_eq!((p.count, p.closed.clone()), (1, vec![false]));

        let two = concat(&[&cube(), &make_box([30.0; 3], [40.0; 3])]).unwrap();
        let p = orientation_patches(&two, &Edges::new(&two));
        assert_eq!((p.count, p.closed.clone()), (2, vec![true, true]));
        assert_eq!(p.patch_of[11], 0);
        assert_eq!(p.patch_of[12], 1);

        // A fin on a cube edge: the edge gets three sides, so the walk stops there; the cube is
        // still one patch (joined round the other edges) but no longer closed.
        let mut finned = cube();
        finned.positions.extend_from_slice(&[10.0, -10.0, -10.0]);
        finned.indices.extend_from_slice(&[0, 1, 8]);
        let p = orientation_patches(&finned, &Edges::new(&finned));
        assert_eq!((p.count, p.closed.clone()), (2, vec![false, false]));
        assert_eq!(p.patch_of[12], 1);
    }

    #[test]
    fn a_mobius_strip_has_one_conflict() {
        let m = mobius(12);
        let p = orientation_patches(&m, &Edges::new(&m));
        assert_eq!(
            (p.count, p.conflicts, p.closed.clone()),
            (1, 1, vec![false])
        );
    }

    #[test]
    fn turning_the_flagged_triangles_leaves_nothing_to_turn() {
        // Applying the flips must give a patch whose walk asks for no more flips, even on a
        // Möbius strip, where some edge always disagrees: the walk must not depend on the side
        // order that turning a triangle changes. Try many strips with scrambled windings.
        for (n, wide) in [(5u32, false), (8, false), (5, true), (8, true), (13, true)] {
            for seed in 0..40u32 {
                let mut m = if wide { wide_mobius(n) } else { mobius(n) };
                for t in 0..m.triangle_count() {
                    let hash = (t as u32).wrapping_mul(2_654_435_761) ^ seed.wrapping_mul(40_503);
                    if hash.is_multiple_of(3) {
                        flip(&mut m, t);
                    }
                }
                let p = orientation_patches(&m, &Edges::new(&m));
                for t in 0..m.triangle_count() {
                    if p.flip[t] {
                        flip(&mut m, t);
                    }
                }
                let again = orientation_patches(&m, &Edges::new(&m));
                assert!(again.flip.iter().all(|&f| !f), "n {n} seed {seed}");
                assert_eq!(again.conflicts, p.conflicts);
            }
        }
        // A tab glued to the strip's border as triangle 0, winding against its neighbour: the
        // walk starts at the tab and splits into its two directions round the strip at that
        // neighbour, which gets turned. Where the two directions meet (the conflict edge) must
        // not move when the neighbour's sides are renumbered by turning it.
        for n in [6u32, 7, 12] {
            let strip = mobius(n);
            let mut m = strip.clone();
            m.positions.extend_from_slice(&[4.0, 0.0, -3.0]);
            let tab = strip.vertex_count() as u32;
            // The strip's first triangle is (0, 1, 3): its side 1→3 is a border edge.
            m.indices = [vec![1, 3, tab], strip.indices.clone()].concat();
            let p = orientation_patches(&m, &Edges::new(&m));
            assert!(p.flip[1], "the tab's neighbour turns");
            for t in 0..m.triangle_count() {
                if p.flip[t] {
                    flip(&mut m, t);
                }
            }
            let again = orientation_patches(&m, &Edges::new(&m));
            assert!(again.flip.iter().all(|&f| !f), "n {n}");
            assert_eq!(again.conflicts, 1);
        }
        // The same on an ordinary cube with scrambled windings: one pass makes it consistent.
        let mut m = cube();
        for t in [1, 2, 6, 7, 11] {
            flip(&mut m, t);
        }
        let p = orientation_patches(&m, &Edges::new(&m));
        for t in 0..12 {
            if p.flip[t] {
                flip(&mut m, t);
            }
        }
        let edges = Edges::new(&m);
        assert!((0..edges.len() as u32).all(|e| is_consistent(&m, &edges, e)));
    }

    #[test]
    fn a_repeated_index_triangle_neither_joins_nor_opens() {
        // Triangle (0, 0, 1) alone: its two real sides are each other's twin; its zero-length side
        // is no edge, so the patch counts as closed (and has no volume).
        let m = mesh(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0], &[0, 0, 1]);
        let edges = Edges::new(&m);
        assert_eq!(edges.twin(1), 2);
        let p = orientation_patches(&m, &edges);
        assert_eq!((p.count, p.conflicts, p.closed.clone()), (1, 0, vec![true]));
    }

    #[test]
    fn keep_triangles_keeps_order_and_drops_unused_vertices() {
        let m = cube();
        // Triangles (4, 5, 6), (4, 6, 7) and (1, 6, 5).
        let keep: Vec<bool> = (0..12).map(|t| t == 2 || t == 3 || t == 9).collect();
        let (kept, origin) = keep_triangles(&m, &keep);
        assert_eq!(origin, vec![2, 3, 9]);
        // Vertices 1, 4, 5, 6, 7 remain, renumbered 0..5 in their order.
        assert_eq!(
            kept.positions,
            vec![
                20.0, 0.0, 0.0, 0.0, 0.0, 20.0, 20.0, 0.0, 20.0, 20.0, 20.0, 20.0, 0.0, 20.0, 20.0,
            ]
        );
        assert_eq!(kept.indices, vec![1, 2, 3, 1, 3, 4, 0, 3, 2]);
        let (none, origin) = keep_triangles(&m, &[false; 12]);
        assert_eq!((none, origin), (Mesh::default(), vec![]));
        let (all, origin) = keep_triangles(&m, &[true; 12]);
        assert_eq!(all, m);
        assert_eq!(origin, (0..12).collect::<Vec<u32>>());
    }

    #[test]
    fn bbox_diagonal_counts_only_used_vertices() {
        assert!((bbox_diagonal(&cube()) - 20.0 * 3f64.sqrt()).abs() < 1e-12);
        assert_eq!(bbox_diagonal(&Mesh::default()), 0.0);
        let mut m = cube();
        m.positions.extend_from_slice(&[500.0, 500.0, 500.0]);
        assert!((bbox_diagonal(&m) - 20.0 * 3f64.sqrt()).abs() < 1e-12);
    }
}
