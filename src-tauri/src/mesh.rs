//! Indexed triangle meshes in millimetres: welding, transforms and the measurements the info panel
//! shows. Plain data and functions only, so all of it is testable without Tauri.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

/// Column-major 4x4 local-to-world matrix, the layout three.js and the frontend store use.
pub type Matrix = [f64; 16];

pub const IDENTITY: Matrix = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0,
];

/// Part-local triangle mesh, the same shape as `MeshData` in src/store.ts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    /// x, y, z per vertex.
    pub positions: Vec<f32>,
    /// Three vertex indices per triangle.
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn vertex_count(&self) -> usize {
        self.positions.len() / 3
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Checks what every other function here relies on. Meshes that come from outside the Rust
    /// code (the frontend) go through this first; the readers build meshes that satisfy it.
    pub fn check(&self) -> Result<(), String> {
        if !self.positions.len().is_multiple_of(3) {
            return Err("its vertex list is incomplete".into());
        }
        if !self.indices.len().is_multiple_of(3) {
            return Err("its triangle list is incomplete".into());
        }
        if self.indices.len() >= u32::MAX as usize {
            return Err("it has more triangles than tricky3D can handle".into());
        }
        let count = self.vertex_count();
        if self.indices.iter().any(|&i| i as usize >= count) {
            return Err("a triangle refers to a vertex that does not exist".into());
        }
        if self.positions.iter().any(|v| !v.is_finite()) {
            return Err("it has coordinates that are not numbers".into());
        }
        Ok(())
    }

    pub(crate) fn point(&self, index: u32) -> [f64; 3] {
        let i = index as usize * 3;
        [
            self.positions[i] as f64,
            self.positions[i + 1] as f64,
            self.positions[i + 2] as f64,
        ]
    }

    pub fn vertex(&self, index: u32) -> [f32; 3] {
        let i = index as usize * 3;
        [
            self.positions[i],
            self.positions[i + 1],
            self.positions[i + 2],
        ]
    }
}

// ---------------------------------------------------------------------------------------------
// Matrices

/// `a * b` for column-major matrices: applies `b` first, then `a`.
pub fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    out
}

/// Applies the affine part of `m` (the bottom row is ignored, as three.js does for meshes).
pub fn transform_point(m: &Matrix, p: [f64; 3]) -> [f64; 3] {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

/// Determinant of the 3x3 linear part; negative for a mirroring transform.
pub fn linear_determinant(m: &Matrix) -> f64 {
    m[0] * (m[5] * m[10] - m[9] * m[6]) - m[4] * (m[1] * m[10] - m[9] * m[2])
        + m[8] * (m[1] * m[6] - m[5] * m[2])
}

/// The mesh with `m` baked into its vertices (f64 math, one rounding to f32 per coordinate). A
/// mirroring matrix also reverses each triangle's winding so the surface keeps facing outwards.
pub fn bake(mesh: &Mesh, m: &Matrix) -> Mesh {
    if *m == IDENTITY {
        return mesh.clone();
    }
    let mut positions = Vec::with_capacity(mesh.positions.len());
    for p in mesh.positions.as_chunks::<3>().0 {
        let q = transform_point(m, [p[0] as f64, p[1] as f64, p[2] as f64]);
        positions.extend_from_slice(&[q[0] as f32, q[1] as f32, q[2] as f32]);
    }
    let mut indices = mesh.indices.clone();
    if linear_determinant(m) < 0.0 {
        for t in indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
    }
    Mesh { positions, indices }
}

/// The mesh with the vertices that share a position joined, as import welds a file (`Welder`):
/// each becomes the first vertex at its position, and the triangles keep their order. A bake can
/// move vertices that were apart onto one position, since f32 has fewer steps far from the origin.
/// Same as `weldMesh` in src/ops/bake.ts. None when every vertex has a position of its own.
pub fn join_coincident(mesh: &Mesh) -> Option<Mesh> {
    let mut welder = Welder::new(mesh.vertex_count() * 5);
    let id: Vec<u32> = mesh
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| welder.vertex(*p))
        .collect();
    if welder.positions.len() == mesh.positions.len() {
        return None;
    }
    Some(Mesh {
        positions: welder.positions,
        indices: mesh.indices.iter().map(|&v| id[v as usize]).collect(),
    })
}

// ---------------------------------------------------------------------------------------------
// Welding

const EMPTY: u32 = u32::MAX;

/// Exact-position weld: equal f32 bit patterns become one vertex (-0.0 counts as +0.0), vertices
/// keep the order of their first use, and every triangle is kept, degenerate ones included.
///
/// An open-addressing hash table of vertex ids (linear probing, kept at most half full), so
/// welding millions of corners does no allocation per corner.
pub struct Welder {
    slots: Vec<u32>,
    mask: usize,
    positions: Vec<f32>,
    indices: Vec<u32>,
    not_finite: bool,
    too_many: bool,
}

impl Welder {
    /// `corners` is the expected number of triangle corners (3 per triangle); only a size hint.
    pub fn new(corners: usize) -> Welder {
        // A closed mesh has about one vertex per six corners; leave room so the table rarely grows.
        let expected = (corners / 5).max(64);
        let slots = (expected * 2).next_power_of_two();
        Welder {
            slots: vec![EMPTY; slots],
            mask: slots - 1,
            positions: Vec::with_capacity(expected.min(1 << 26) * 3),
            indices: Vec::with_capacity(corners.min(3 << 26)),
            not_finite: false,
            too_many: false,
        }
    }

    /// The welded index of a position, adding it when it is new.
    #[inline]
    pub fn vertex(&mut self, p: [f32; 3]) -> u32 {
        let key = [bits(p[0]), bits(p[1]), bits(p[2])];
        let mut slot = hash(key) as usize & self.mask;
        loop {
            let id = self.slots[slot];
            if id == EMPTY {
                return self.insert(slot, key);
            }
            let at = id as usize * 3;
            if self.positions[at].to_bits() == key[0]
                && self.positions[at + 1].to_bits() == key[1]
                && self.positions[at + 2].to_bits() == key[2]
            {
                return id;
            }
            slot = (slot + 1) & self.mask;
        }
    }

    #[cold]
    fn insert(&mut self, slot: usize, key: [u32; 3]) -> u32 {
        let id = (self.positions.len() / 3) as u32;
        if id >= EMPTY - 1 {
            self.too_many = true;
            return 0;
        }
        let p = key.map(f32::from_bits);
        if !(p[0].is_finite() && p[1].is_finite() && p[2].is_finite()) {
            self.not_finite = true;
        }
        self.positions.extend_from_slice(&p);
        self.slots[slot] = id;
        if (id as usize + 1) * 2 > self.slots.len() {
            self.grow();
        }
        id
    }

    fn grow(&mut self) {
        let size = self.slots.len() * 2;
        self.slots = vec![EMPTY; size];
        self.mask = size - 1;
        for (id, p) in self.positions.as_chunks::<3>().0.iter().enumerate() {
            let mut slot =
                hash([p[0].to_bits(), p[1].to_bits(), p[2].to_bits()]) as usize & self.mask;
            while self.slots[slot] != EMPTY {
                slot = (slot + 1) & self.mask;
            }
            self.slots[slot] = id as u32;
        }
    }

    #[inline]
    pub fn triangle(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3]) {
        let a = self.vertex(a);
        let b = self.vertex(b);
        let c = self.vertex(c);
        self.indices.extend_from_slice(&[a, b, c]);
    }

    /// Adds a triangle of already-welded vertex ids (from `vertex`).
    #[inline]
    pub fn triangle_ids(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend_from_slice(&[a, b, c]);
    }

    pub fn finish(self) -> Result<Mesh, String> {
        if self.not_finite {
            return Err("it has coordinates that are not numbers".into());
        }
        if self.too_many || self.indices.len() >= u32::MAX as usize {
            return Err("it has more vertices than tricky3D can handle".into());
        }
        Ok(Mesh {
            positions: self.positions,
            indices: self.indices,
        })
    }
}

/// The f32 bits used as the weld key, with -0.0 folded into +0.0.
#[inline]
fn bits(v: f32) -> u32 {
    let b = v.to_bits();
    if b == 0x8000_0000 { 0 } else { b }
}

/// 64-bit mix of the three coordinate bit patterns (murmur3 finaliser), so float grids with
/// zeroed low mantissa bits still spread over the table.
#[inline]
fn hash(k: [u32; 3]) -> u64 {
    let mut h =
        (k[0] as u64 | (k[1] as u64) << 32) ^ (k[2] as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h = h.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    h ^ (h >> 33)
}

/// Welds an indexed mesh whose positions may repeat. Each input vertex is hashed once; vertices
/// no triangle uses are dropped. Indices must be in range.
pub fn weld_indexed(positions: &[f32], indices: &[u32]) -> Result<Mesh, String> {
    let count = positions.len() / 3;
    let mut welder = Welder::new(indices.len());
    let mut remap = vec![EMPTY; count];
    let mut ids = [0u32; 3];
    for t in indices.as_chunks::<3>().0 {
        for (slot, &i) in ids.iter_mut().zip(t) {
            let i = i as usize;
            if i >= count {
                return Err("a triangle refers to a vertex that does not exist".into());
            }
            if remap[i] == EMPTY {
                remap[i] =
                    welder.vertex([positions[i * 3], positions[i * 3 + 1], positions[i * 3 + 2]]);
            }
            *slot = remap[i];
        }
        welder.triangle_ids(ids[0], ids[1], ids[2]);
    }
    welder.finish()
}

// ---------------------------------------------------------------------------------------------
// Measurements

/// What the info panel shows for one mesh, in part-local millimetres. Field names match
/// `MeshStats` in src/io/ipc.ts.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshStats {
    pub triangles: u64,
    pub vertices: u64,
    /// mm³, signed-tetrahedron sum: positive for a closed mesh whose triangles face outwards.
    pub volume: f64,
    /// mm².
    pub area: f64,
    /// Triangles joined across edges with exactly two triangles facing the same way
    /// (`topology::Link::Shell`), as the reference tool counts shells.
    pub shells: u64,
    /// Edges used by exactly one triangle.
    pub open_edges: u64,
    /// Edges used by three or more triangles.
    pub non_manifold_edges: u64,
}

pub struct Analysis {
    pub stats: MeshStats,
    /// x, y, z, x, y, z per edge not shared by exactly two triangles (the viewport's problem
    /// edges, same as `problemEdgePositions` in src/viewport/edges.ts).
    pub problem_edges: Vec<f32>,
}

/// Measures a mesh that passed `Mesh::check`. `progress` gets 0..1 for this mesh.
pub fn analyze(mesh: &Mesh, progress: &mut dyn FnMut(f64)) -> Analysis {
    measure(mesh, progress, &|| false).expect("an analysis nothing stops finishes")
}

/// `analyze` for a repair run's answer, which gives up (None) between its steps once `cancel` is
/// set: on a big result it takes longer than many repair actions do.
pub fn analyze_unless_cancelled(mesh: &Mesh, cancel: &AtomicBool) -> Option<Analysis> {
    measure(mesh, &mut |_| {}, &|| cancel.load(Ordering::Relaxed))
}

fn measure(
    mesh: &Mesh,
    progress: &mut dyn FnMut(f64),
    stop: &dyn Fn() -> bool,
) -> Option<Analysis> {
    if stop() {
        return None;
    }
    let (volume, area) = volume_and_area(mesh);
    progress(0.2);
    if stop() {
        return None;
    }
    let mut open_edges = 0u64;
    let mut non_manifold_edges = 0u64;
    let mut problem_edges = Vec::new();
    for_each_edge(mesh, |lo, hi, uses| {
        if uses == 2 {
            return;
        }
        if uses == 1 {
            open_edges += 1;
        } else {
            non_manifold_edges += 1;
        }
        for v in [lo, hi] {
            let i = v as usize * 3;
            problem_edges.extend_from_slice(&mesh.positions[i..i + 3]);
        }
    });
    progress(0.7);
    if stop() {
        return None;
    }
    let shells = shell_count(mesh);
    progress(1.0);
    Some(Analysis {
        stats: MeshStats {
            triangles: mesh.triangle_count() as u64,
            vertices: mesh.vertex_count() as u64,
            volume,
            area,
            shells,
            open_edges,
            non_manifold_edges,
        },
        problem_edges,
    })
}

pub fn stats(mesh: &Mesh) -> MeshStats {
    analyze(mesh, &mut |_| {}).stats
}

fn volume_and_area(mesh: &Mesh) -> (f64, f64) {
    let mut volume = 0.0;
    let mut area = 0.0;
    for t in mesh.indices.as_chunks::<3>().0 {
        let a = mesh.point(t[0]);
        let b = mesh.point(t[1]);
        let c = mesh.point(t[2]);
        volume += dot(a, cross(b, c));
        let n = cross(sub(b, a), sub(c, a));
        area += dot(n, n).sqrt();
    }
    (volume / 6.0, area / 2.0)
}

/// Calls `each(lower vertex, higher vertex, triangle count)` once per undirected edge, skipping
/// zero-length edges (a triangle that repeats an index). Counting sort by lower vertex, then a
/// tiny sort per bucket: linear time and no hash map, which matters for millions of triangles.
fn for_each_edge(mesh: &Mesh, mut each: impl FnMut(u32, u32, u32)) {
    let count = mesh.vertex_count();
    let mut start = vec![0u32; count + 1];
    for t in mesh.indices.as_chunks::<3>().0 {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            if a != b {
                start[a.min(b) as usize + 1] += 1;
            }
        }
    }
    for v in 0..count {
        start[v + 1] += start[v];
    }
    let mut fill = start[..count].to_vec();
    let mut higher = vec![0u32; start[count] as usize];
    for t in mesh.indices.as_chunks::<3>().0 {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            if a != b {
                let (lo, hi) = (a.min(b) as usize, a.max(b));
                higher[fill[lo] as usize] = hi;
                fill[lo] += 1;
            }
        }
    }
    drop(fill);
    for lo in 0..count {
        let bucket = &mut higher[start[lo] as usize..start[lo + 1] as usize];
        sort_small(bucket);
        let mut i = 0;
        while i < bucket.len() {
            let mut j = i + 1;
            while j < bucket.len() && bucket[j] == bucket[i] {
                j += 1;
            }
            each(lo as u32, bucket[i], (j - i) as u32);
            i = j;
        }
    }
}

/// A bucket holds one vertex's few edges, so insertion sort; big fans use the library sort.
fn sort_small(v: &mut [u32]) {
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

/// The shells (`topology::Link::Shell`).
fn shell_count(mesh: &Mesh) -> u64 {
    use crate::repair::topology::{Edges, Link, components};
    components(mesh, &Edges::new(mesh), Link::Shell).1 as u64
}

/// Union-find over shared vertex indices (path halving): the root of every vertex once the
/// corners of each triangle are joined. A component's root is its lowest vertex index; a vertex
/// no triangle uses is its own root.
pub(crate) fn vertex_roots(mesh: &Mesh) -> Vec<u32> {
    let count = mesh.vertex_count();
    let mut parent: Vec<u32> = (0..count as u32).collect();
    fn root(parent: &mut [u32], mut v: u32) -> u32 {
        while parent[v as usize] != v {
            let up = parent[parent[v as usize] as usize];
            parent[v as usize] = up;
            v = up;
        }
        v
    }
    for t in mesh.indices.as_chunks::<3>().0 {
        for (a, b) in [(t[0], t[1]), (t[1], t[2])] {
            let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
            if ra != rb {
                parent[ra.max(rb) as usize] = ra.min(rb);
            }
        }
    }
    for v in 0..count as u32 {
        let r = root(&mut parent, v);
        parent[v as usize] = r;
    }
    parent
}

pub(crate) fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(crate) fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Unit normal of a triangle for STL output; zero for a triangle with no area.
pub fn triangle_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let f = |p: [f32; 3]| [p[0] as f64, p[1] as f64, p[2] as f64];
    let n = cross(sub(f(b), f(a)), sub(f(c), f(a)));
    let length = dot(n, n).sqrt();
    if length == 0.0 || !length.is_finite() {
        return [0.0; 3];
    }
    [
        (n[0] / length) as f32,
        (n[1] / length) as f32,
        (n[2] / length) as f32,
    ]
}

/// Axis-aligned bounds `[min, max]` of the vertices triangles use; None for an empty mesh.
pub fn bounds(mesh: &Mesh) -> Option<[[f32; 3]; 2]> {
    let mut out: Option<[[f32; 3]; 2]> = None;
    for &i in &mesh.indices {
        let i = i as usize * 3;
        let p = [
            mesh.positions[i],
            mesh.positions[i + 1],
            mesh.positions[i + 2],
        ];
        let b = out.get_or_insert([p, p]);
        for k in 0..3 {
            b[0][k] = b[0][k].min(p[k]);
            b[1][k] = b[1][k].max(p[k]);
        }
    }
    out
}

/// Thousands separators for counts in messages: 1200 → "1,200".
pub fn fmt_count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// A closed axis-aligned box from `min` to `max` with outward-facing triangles (12 triangles).
pub fn make_box(min: [f32; 3], max: [f32; 3]) -> Mesh {
    let [x0, y0, z0] = min;
    let [x1, y1, z1] = max;
    let positions = vec![
        x0, y0, z0, x1, y0, z0, x1, y1, z0, x0, y1, z0, // bottom
        x0, y0, z1, x1, y0, z1, x1, y1, z1, x0, y1, z1, // top
    ];
    let indices = vec![
        0, 2, 1, 0, 3, 2, // -z
        4, 5, 6, 4, 6, 7, // +z
        0, 1, 5, 0, 5, 4, // -y
        2, 3, 7, 2, 7, 6, // +y
        1, 2, 6, 1, 6, 5, // +x
        3, 0, 4, 3, 4, 7, // -x
    ];
    Mesh { positions, indices }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> Mesh {
        make_box([0.0; 3], [20.0; 3])
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn a_cancel_stops_the_analysis_of_a_repair_answer() {
        let mesh = cube();
        let run = AtomicBool::new(false);
        let a = analyze_unless_cancelled(&mesh, &run).unwrap();
        assert_eq!(a.stats, stats(&mesh));
        assert_eq!(a.problem_edges, analyze(&mesh, &mut |_| {}).problem_edges);
        assert!(analyze_unless_cancelled(&mesh, &AtomicBool::new(true)).is_none());
    }

    #[test]
    fn cube_known_values() {
        let s = stats(&cube());
        assert!(close(s.volume, 8000.0), "volume {}", s.volume);
        assert!(close(s.area, 2400.0), "area {}", s.area);
        assert_eq!(
            (
                s.triangles,
                s.vertices,
                s.shells,
                s.open_edges,
                s.non_manifold_edges
            ),
            (12, 8, 1, 0, 0)
        );
    }

    #[test]
    fn cube_missing_a_face_has_four_open_edges() {
        let mut mesh = cube();
        mesh.indices.drain(0..6);
        let a = analyze(&mesh, &mut |_| {});
        assert_eq!(a.stats.open_edges, 4);
        assert_eq!(a.stats.non_manifold_edges, 0);
        assert_eq!(a.problem_edges.len(), 4 * 6);
        // Only the open bottom square's corners (z = 0) appear in the problem edges.
        assert!(
            a.problem_edges
                .as_chunks::<3>()
                .0
                .iter()
                .all(|p| p[2] == 0.0)
        );
    }

    #[test]
    fn two_disjoint_cubes_are_two_shells() {
        let a = make_box([0.0; 3], [1.0; 3]);
        let b = make_box([5.0; 3], [6.0; 3]);
        let mut positions = a.positions.clone();
        positions.extend_from_slice(&b.positions);
        let mut indices = a.indices.clone();
        indices.extend(b.indices.iter().map(|i| i + 8));
        let s = stats(&Mesh { positions, indices });
        assert_eq!(s.shells, 2);
        assert!(close(s.volume, 2.0));
    }

    #[test]
    fn three_triangles_on_one_edge_are_non_manifold() {
        // Three fins sharing the edge 0-1.
        let positions = vec![
            0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 0.0,
        ];
        let indices = vec![0, 1, 2, 0, 1, 3, 0, 1, 4];
        let a = analyze(&Mesh { positions, indices }, &mut |_| {});
        assert_eq!(a.stats.non_manifold_edges, 1);
        assert_eq!(a.stats.open_edges, 6);
        assert_eq!(a.problem_edges.len(), 7 * 6);
    }

    #[test]
    fn unused_vertices_do_not_count_as_shells_and_degenerate_edges_are_ignored() {
        let mut mesh = cube();
        mesh.positions.extend_from_slice(&[99.0, 99.0, 99.0]);
        mesh.indices.extend_from_slice(&[0, 0, 1]);
        let s = stats(&mesh);
        assert_eq!(s.vertices, 9);
        // The degenerate triangle adds a third use of edge 0-1 (from 0-1 and 1-0), so it is a
        // shell of its own beside the cube, which stays one across its other edges.
        assert_eq!(s.non_manifold_edges, 1);
        assert_eq!(s.shells, 2);
    }

    #[test]
    fn join_coincident_joins_only_vertices_at_one_position() {
        let box_ = cube();
        assert_eq!(join_coincident(&box_), None);
        // 0 and 2^-23 are apart; moved by 2^24 both land on 2^24 (the f32 step there is 2).
        let mesh = Mesh {
            positions: vec![
                0.0,
                0.0,
                0.0,
                1.0 / 8_388_608.0,
                0.0,
                0.0,
                0.0,
                1.0,
                0.0,
                0.0,
                0.0,
                1.0,
            ],
            indices: vec![0, 2, 3, 1, 3, 2, 0, 1, 2],
        };
        assert_eq!(join_coincident(&mesh), None);
        let mut m = IDENTITY;
        m[12] = 16_777_216.0;
        let joined = join_coincident(&bake(&mesh, &m)).unwrap();
        let far = 16_777_216.0;
        assert_eq!(
            joined.positions,
            vec![far, 0.0, 0.0, far, 1.0, 0.0, far, 0.0, 1.0]
        );
        assert_eq!(joined.indices, vec![0, 1, 2, 0, 2, 1, 0, 0, 1]);
    }

    #[test]
    fn weld_merges_equal_positions_and_negative_zero() {
        let mut w = Welder::new(9);
        w.triangle([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        w.triangle([-0.0, 0.0, -0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]);
        w.triangle([2.0, 2.0, 2.0], [2.0, 2.0, 2.0], [2.0, 2.0, 2.0]);
        let mesh = w.finish().unwrap();
        assert_eq!(mesh.vertex_count(), 4);
        assert_eq!(mesh.indices, vec![0, 1, 2, 0, 2, 1, 3, 3, 3]);
        assert_eq!(mesh.positions[0].to_bits(), 0);
    }

    #[test]
    fn weld_survives_table_growth() {
        let mut w = Welder::new(0);
        for i in 0..10_000 {
            let f = i as f32;
            w.triangle([f, 0.0, 0.0], [f, 1.0, 0.0], [f, 0.0, 1.0]);
        }
        for i in 0..10_000 {
            let f = i as f32;
            w.triangle([f, 0.0, 0.0], [f, 1.0, 0.0], [f, 0.0, 1.0]);
        }
        let mesh = w.finish().unwrap();
        assert_eq!(mesh.vertex_count(), 30_000);
        assert_eq!(mesh.indices[..30_000], mesh.indices[30_000..]);
    }

    #[test]
    fn weld_rejects_nan() {
        let mut w = Welder::new(3);
        w.triangle([f32::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(w.finish().is_err());
    }

    #[test]
    fn weld_indexed_drops_unused_and_checks_indices() {
        let positions = [
            0.0, 0.0, 0.0, 9.0, 9.0, 9.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0,
        ];
        let mesh = weld_indexed(&positions, &[0, 2, 3, 4, 3, 0]).unwrap();
        assert_eq!(mesh.vertex_count(), 3);
        assert_eq!(mesh.indices, vec![0, 1, 2, 1, 2, 0]);
        assert!(weld_indexed(&positions, &[0, 1, 7]).is_err());
    }

    #[test]
    fn bake_applies_matrix_and_flips_mirrored_winding() {
        let mut m = IDENTITY;
        m[0] = -1.0; // mirror x
        m[12] = 5.0;
        let baked = bake(&cube(), &m);
        let s = stats(&baked);
        assert!(close(s.volume, 8000.0));
        assert_eq!(bounds(&baked), Some([[-15.0, 0.0, 0.0], [5.0, 20.0, 20.0]]));
    }

    #[test]
    fn multiply_composes_in_order() {
        let mut scale = IDENTITY;
        scale[0] = 2.0;
        let mut shift = IDENTITY;
        shift[12] = 1.0;
        // shift * scale: scale first, then shift.
        let m = multiply(&shift, &scale);
        assert_eq!(transform_point(&m, [1.0, 0.0, 0.0]), [3.0, 0.0, 0.0]);
    }

    #[test]
    fn counts_get_separators() {
        assert_eq!(fmt_count(0), "0");
        assert_eq!(fmt_count(999), "999");
        assert_eq!(fmt_count(1200), "1,200");
        assert_eq!(fmt_count(5_000_000), "5,000,000");
    }
}
