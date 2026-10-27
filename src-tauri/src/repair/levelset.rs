//! Wrap part surface's last resort: the part's solid sampled on a grid and its boundary built anew
//! by marching tetrahedra. Closed, oriented and free of crossings by construction, whatever the
//! input:
//! - each grid point is inside or outside by its winding number along its grid line in x, from
//!   the points where the line meets the triangles; a line through an edge or a corner counts as
//!   if moved aside by an infinitesimal (ε, ε²) in (y, z), so every point gets exactly one answer;
//! - every cube of the grid splits into six tetrahedra round its main diagonal (Kuhn), the same
//!   way in every cube, so neighbouring tetrahedra share whole faces. Between the inside and the
//!   outside corners of each tetrahedron lies one triangle or one quad (two triangles), so the
//!   boundary of any set of grid points is a closed manifold, and each piece lies in its own
//!   tetrahedron, touching its neighbours only along the edges they share;
//! - each vertex sits where its grid edge meets the part's surface (the meeting nearest the
//!   outside end), kept clear of the edge's ends. The surface stays where it was; edges, corners
//!   and features smaller than a cell are softened.

use rayon::prelude::*;

use super::bvh::{self, Aabb, Bvh, boxes_of};
use super::exact::{self, P3, cross, dot, orient2d, project, sub};
use super::{Control, RepairResult};
use crate::mesh::Mesh;

/// Most grid points sampled (the inside map holds a bit per point).
const MAX_POINTS: f64 = 2.0e9;

/// Output triangles aimed for, per input triangle and at least and at most.
const TRIANGLES_PER_INPUT: f64 = 8.0;
const MIN_TRIANGLES: f64 = 500_000.0;
const MAX_TRIANGLES: f64 = 2_000_000.0;

/// Marching tetrahedra makes about this many triangles per square cell of surface.
const TRIANGLES_PER_CELL_AREA: f64 = 3.5;

/// The finest grid step, mm: vertices kept `CLEAR` from their edges' ends stay further apart than
/// Remove degenerate faces' 0.002 mm tolerance.
pub(crate) const MIN_CELL: f64 = 0.01;
const CLEAR: f64 = 0.004;
/// The least clearance as a fraction of the edge.
const CLEAR_MIN: f64 = 0.05;

pub(crate) struct Remeshed {
    pub mesh: Mesh,
    /// The grid step, mm.
    pub cell: f64,
}

/// How many triangles rebuilding a surface of `triangles` triangles aims for:
/// `TRIANGLES_PER_INPUT` per input triangle, within `MIN_TRIANGLES`..`MAX_TRIANGLES`.
pub(crate) fn target_triangles(triangles: usize) -> usize {
    (triangles as f64 * TRIANGLES_PER_INPUT).clamp(MIN_TRIANGLES, MAX_TRIANGLES) as usize
}

/// The grid step for rebuilding a surface of `area` mm² in about `target` triangles, no finer than
/// `MIN_CELL` and coarse enough that `bounds` holds at most `MAX_POINTS` grid points.
pub(crate) fn cell_for(area: f64, target: usize, bounds: &Aabb) -> f64 {
    let target = target as f64;
    let mut cell = (TRIANGLES_PER_CELL_AREA * area / target)
        .sqrt()
        .max(MIN_CELL);
    let points = |cell: f64| {
        (0..3)
            .map(|a| ((bounds[1][a] - bounds[0][a]) / cell + 5.0).ceil())
            .product::<f64>()
    };
    while points(cell) > MAX_POINTS {
        cell *= 1.25;
    }
    cell
}

/// The area of the triangles `faces` of `mesh`, mm².
pub(crate) fn area_of(mesh: &Mesh, faces: impl Iterator<Item = u32>) -> f64 {
    faces
        .map(|t| {
            let [a, b, c] = exact::corners(mesh, t);
            exact::length(cross(sub(b, a), sub(c, a))) / 2.0
        })
        .sum()
}

/// The surface of `solid`'s solid (winding number not zero) inside `within` (default: all of it;
/// the grid covers the boxes' union and points outside every box count as outside), rebuilt on a
/// grid of step `cell`. None when there is nothing inside.
pub(crate) fn remesh(
    solid: &Mesh,
    within: Option<&[Aabb]>,
    cell: f64,
    control: &mut Control,
) -> RepairResult<Option<Remeshed>> {
    let points: Vec<P3> = (0..solid.vertex_count() as u32)
        .map(|v| exact::vertex(solid, v))
        .collect();
    let triangles: Vec<[u32; 3]> = solid.indices.as_chunks::<3>().0.to_vec();
    let mut bounds = bvh::EMPTY;
    match within {
        Some(boxes) => boxes.iter().for_each(|b| bvh::grow(&mut bounds, b)),
        None => points.iter().for_each(|&p| bvh::include(&mut bounds, p)),
    }
    if triangles.is_empty() || bounds[0][0] > bounds[1][0] {
        return Ok(None);
    }
    let grid = Grid::new(&bounds, cell);
    control.check()?;
    let inside = grid.classify(&points, &triangles, within, control.cancel_flag());
    control.check()?;
    control.progress(0.4);
    let (tris, quads) = grid.pieces(&inside, control.cancel_flag());
    control.check()?;
    if tris.is_empty() && quads.is_empty() {
        return Ok(None);
    }
    control.progress(0.6);
    let mut keys: Vec<u64> = tris
        .par_iter()
        .flatten()
        .chain(quads.par_iter().flatten())
        .copied()
        .collect();
    keys.par_sort_unstable();
    keys.dedup();
    let bvh = Bvh::new(&boxes_of(&points, &triangles));
    let positions: Vec<P3> = keys
        .par_iter()
        .map(|&key| grid.vertex(key, &inside, &points, &triangles, &bvh))
        .collect();
    control.check()?;
    control.progress(0.9);
    let id = |key: u64| keys.binary_search(&key).unwrap() as u32;
    let mut indices: Vec<u32> = Vec::with_capacity(3 * (tris.len() + 2 * quads.len()));
    for t in &tris {
        indices.extend(t.map(id));
    }
    for q in &quads {
        let [a, b, c, d] = q.map(id);
        let at = |v: u32| positions[v as usize];
        let ac = exact::length(sub(at(c), at(a)));
        let bd = exact::length(sub(at(d), at(b)));
        if ac <= bd {
            indices.extend([a, b, c, a, c, d]);
        } else {
            indices.extend([a, b, d, b, c, d]);
        }
    }
    let positions: Vec<f32> = positions.iter().flatten().map(|&x| x as f32).collect();
    control.progress(1.0);
    Ok(Some(Remeshed {
        mesh: Mesh { positions, indices },
        cell,
    }))
}

/// The six tetrahedra of a cube: the paths from corner 0 to corner 7 (bit 1 is +x, 2 is +y, 4 is
/// +z), one axis at a time.
const TETS: [[u8; 4]; 6] = [
    [0, 1, 3, 7],
    [0, 1, 5, 7],
    [0, 2, 3, 7],
    [0, 2, 6, 7],
    [0, 4, 5, 7],
    [0, 4, 6, 7],
];

struct Grid {
    origin: P3,
    cell: f64,
    /// Points along each axis.
    n: [usize; 3],
    /// u64 words per row of x (each row starts a new word).
    row_words: usize,
}

/// A bit per grid point: rows of x, `ny` rows per slab of z, one slab per z.
struct Inside {
    slabs: Vec<Vec<u64>>,
}

impl Grid {
    fn new(bounds: &Aabb, cell: f64) -> Grid {
        // One clear cell round the bounds, so the border points are all outside; the odd offset
        // keeps the grid off round coordinates, where CAD faces like to lie.
        let pad = 1.5 * cell + 0.137 * cell;
        let origin = bounds[0].map(|x| x - pad);
        let n = [0, 1, 2].map(|a| ((bounds[1][a] - origin[a] + pad) / cell).ceil() as usize + 1);
        Grid {
            origin,
            cell,
            n,
            row_words: n[0].div_ceil(64) + 1,
        }
    }

    fn coord(&self, axis: usize, i: usize) -> f64 {
        self.origin[axis] + i as f64 * self.cell
    }

    fn point(&self, [i, j, k]: [usize; 3]) -> P3 {
        [self.coord(0, i), self.coord(1, j), self.coord(2, k)]
    }

    /// The first index whose coordinate is at least `x`, and the last at most `x`.
    fn range(&self, axis: usize, lo: f64, hi: f64) -> (usize, usize) {
        let first = ((lo - self.origin[axis]) / self.cell).ceil().max(0.0) as usize;
        let last = ((hi - self.origin[axis]) / self.cell).floor();
        let last = if last < 0.0 {
            0
        } else {
            (last as usize).min(self.n[axis] - 1)
        };
        (first, last)
    }

    /// Where the grid's lines along `axis` meet the triangles. The lines are indexed by their grid
    /// indices along `(axis + 1) % 3` (j) and `(axis + 2) % 3` (k); per k, on several threads,
    /// `each(k, crossings)` gets the crossings of its lines as (j, coordinate along `axis`, +1
    /// where the line enters / −1 where it leaves), sorted by j, then along the line. A line
    /// through an edge or a corner counts as moved aside by an infinitesimal (ε, ε²), so every
    /// line meets a closed surface an even number of times.
    fn sweep<R: Send>(
        &self,
        axis: usize,
        points: &[P3],
        triangles: &[[u32; 3]],
        cancel: &std::sync::atomic::AtomicBool,
        each: impl Fn(usize, &[(u32, f64, i32)]) -> R + Sync,
    ) -> Vec<R> {
        let (ja, ka) = ((axis + 1) % 3, (axis + 2) % 3);
        let nk = self.n[ka];
        // Triangles by the slabs of k they span.
        let spans: Vec<Option<(usize, usize)>> = triangles
            .iter()
            .map(|t| {
                let z = t.map(|v| points[v as usize][ka]);
                let (lo, hi) = (z[0].min(z[1]).min(z[2]), z[0].max(z[1]).max(z[2]));
                let (first, last) = self.range(ka, lo, hi);
                (first <= last && lo <= self.coord(ka, last)).then_some((first, last))
            })
            .collect();
        let mut start = vec![0usize; nk + 1];
        for &(first, last) in spans.iter().flatten() {
            for k in first..=last {
                start[k + 1] += 1;
            }
        }
        for k in 0..nk {
            start[k + 1] += start[k];
        }
        let mut fill = start.clone();
        let mut by_slab = vec![0u32; start[nk]];
        for (t, span) in spans.iter().enumerate() {
            if let Some((first, last)) = *span {
                for k in first..=last {
                    by_slab[fill[k]] = t as u32;
                    fill[k] += 1;
                }
            }
        }
        (0..nk)
            .into_par_iter()
            .map(|k| {
                let mut crossings: Vec<(u32, f64, i32)> = Vec::new();
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    return each(k, &crossings);
                }
                let z = self.coord(ka, k);
                for &t in &by_slab[start[k]..start[k + 1]] {
                    let [a, b, c] = triangles[t as usize].map(|v| points[v as usize]);
                    let [pa, pb, pc] = [a, b, c].map(|p| project(p, axis));
                    let turn = orient2d(pa, pb, pc);
                    if turn == 0 {
                        continue;
                    }
                    let lo = pa[0].min(pb[0]).min(pc[0]);
                    let hi = pa[0].max(pb[0]).max(pc[0]);
                    let (first, last) = self.range(ja, lo, hi);
                    if first > last {
                        continue;
                    }
                    let normal = cross(sub(b, a), sub(c, a));
                    for j in first..=last {
                        let q = [self.coord(ja, j), z];
                        if [(pa, pb), (pb, pc), (pc, pa)]
                            .iter()
                            .all(|&(u, v)| side(u, v, q) == turn)
                        {
                            let x = a[axis]
                                - (normal[ja] * (q[0] - a[ja]) + normal[ka] * (q[1] - a[ka]))
                                    / normal[axis];
                            // The normal's component along the line is the projection's turn: a
                            // face turned against the line is where it enters.
                            crossings.push((j as u32, x, -turn));
                        }
                    }
                }
                crossings.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
                each(k, &crossings)
            })
            .collect()
    }

    /// Along one line along `axis` (its crossings, sorted): per stretch inside the solid that holds
    /// no grid point (a wall thinner than a cell, crossed by the line), the grid point nearest its
    /// middle, which then counts as inside so the wall is not lost.
    fn thin(&self, axis: usize, line: &[(u32, f64, i32)]) -> Vec<usize> {
        let mut out = Vec::new();
        let mut winding = 0;
        for w in 0..line.len().saturating_sub(1) {
            winding += line[w].2;
            if winding == 0 {
                continue;
            }
            let (a, b) = (line[w].1, line[w + 1].1);
            let first = ((a - self.origin[axis]) / self.cell).ceil();
            let last = ((b - self.origin[axis]) / self.cell).floor();
            if first > last {
                let mid = ((a + b) / 2.0 - self.origin[axis]) / self.cell;
                out.push(mid.round().clamp(1.0, (self.n[axis] - 2) as f64) as usize);
            }
        }
        out
    }

    /// The inside map: per grid point, whether the winding number of `triangles` there is not
    /// zero, or the point stands for a wall thinner than a cell (`thin`); and, given `within`,
    /// the point lies in one of its boxes.
    fn classify(
        &self,
        points: &[P3],
        triangles: &[[u32; 3]],
        within: Option<&[Aabb]>,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Inside {
        let [nx, ny, nz] = self.n;
        // Lines along x give the winding numbers, row by row.
        let mut slabs: Vec<Vec<u64>> = self.sweep(0, points, triangles, cancel, |_, crossings| {
            let mut bits = vec![0u64; self.row_words * ny];
            for line in crossings.chunk_by(|a, b| a.0 == b.0) {
                let j = line[0].0 as usize;
                let row = &mut bits[j * self.row_words..(j + 1) * self.row_words];
                let (mut winding, mut next) = (0, 0);
                for i in 0..nx {
                    let x = self.coord(0, i);
                    while next < line.len() && line[next].1 < x {
                        winding += line[next].2;
                        next += 1;
                    }
                    if winding != 0 {
                        row[i / 64] |= 1 << (i % 64);
                    }
                    if next == line.len() && winding == 0 {
                        break;
                    }
                }
                for i in self.thin(0, line) {
                    row[i / 64] |= 1 << (i % 64);
                }
            }
            bits
        });
        // Walls thin across y and z.
        for axis in [1, 2] {
            let (ja, ka) = ((axis + 1) % 3, (axis + 2) % 3);
            let marks: Vec<Vec<[usize; 3]>> =
                self.sweep(axis, points, triangles, cancel, |k, crossings| {
                    let mut out = Vec::new();
                    for line in crossings.chunk_by(|a, b| a.0 == b.0) {
                        for i in self.thin(axis, line) {
                            let mut p = [0usize; 3];
                            p[axis] = i;
                            p[ja] = line[0].0 as usize;
                            p[ka] = k;
                            out.push(p);
                        }
                    }
                    out
                });
            for [i, j, k] in marks.into_iter().flatten() {
                slabs[k][j * self.row_words + i / 64] |= 1 << (i % 64);
            }
        }
        slabs.par_iter_mut().enumerate().for_each(|(k, bits)| {
            // The border points are outside, so the surface closes inside the grid even where an
            // open input leaves a line inside to its end.
            if k == 0 || k == nz - 1 {
                bits.fill(0);
            }
            for j in [0, ny - 1] {
                bits[j * self.row_words..(j + 1) * self.row_words].fill(0);
            }
            for j in 0..ny {
                for i in [0, nx - 1] {
                    bits[j * self.row_words + i / 64] &= !(1 << (i % 64));
                }
            }
            if let Some(boxes) = within {
                // The points in some box: each box's ranges of i and j, set in a mask.
                let z = self.coord(2, k);
                let mut mask = vec![0u64; bits.len()];
                for b in boxes.iter().filter(|b| b[0][2] <= z && z <= b[1][2]) {
                    let (i0, i1) = self.range(0, b[0][0], b[1][0]);
                    let (j0, j1) = self.range(1, b[0][1], b[1][1]);
                    if i0 > i1 || j0 > j1 {
                        continue;
                    }
                    for j in j0..=j1 {
                        let row = &mut mask[j * self.row_words..(j + 1) * self.row_words];
                        let words = row.iter_mut().enumerate().take(i1 / 64 + 1).skip(i0 / 64);
                        for (w, word) in words {
                            let from = if w == i0 / 64 { i0 % 64 } else { 0 };
                            let to = if w == i1 / 64 { i1 % 64 } else { 63 };
                            *word |= (u64::MAX >> (63 - to)) & (u64::MAX << from);
                        }
                    }
                }
                for (w, m) in bits.iter_mut().zip(&mask) {
                    *w &= m;
                }
            }
        });
        Inside { slabs }
    }

    /// Whether grid point (i, j, k) is inside.
    fn at(&self, inside: &Inside, [i, j, k]: [usize; 3]) -> bool {
        inside.slabs[k][j * self.row_words + i / 64] >> (i % 64) & 1 == 1
    }

    /// The key of the grid edge from point `p` to `p` + the corner offset `d` (bits as in `TETS`).
    fn key(&self, [i, j, k]: [usize; 3], d: u8) -> u64 {
        ((i + self.n[0] * (j + self.n[1] * k)) as u64) << 3 | d as u64
    }

    /// The pieces of surface: triangles and quads, each corner the key of the grid edge it lies
    /// on, turned to face the outside.
    #[allow(clippy::type_complexity)]
    fn pieces(
        &self,
        inside: &Inside,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> (Vec<[u64; 3]>, Vec<[u64; 4]>) {
        let [nx, ny, nz] = self.n;
        let per_slab: Vec<(Vec<[u64; 3]>, Vec<[u64; 4]>)> = (0..nz - 1)
            .into_par_iter()
            .map(|k| {
                let (mut tris, mut quads) = (Vec::new(), Vec::new());
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    return (tris, quads);
                }
                for j in 0..ny - 1 {
                    let rows = [(j, k), (j + 1, k), (j, k + 1), (j + 1, k + 1)].map(|(j, k)| {
                        &inside.slabs[k][j * self.row_words..(j + 1) * self.row_words]
                    });
                    for w in 0..self.row_words - 1 {
                        // Cubes from x = 64w to 64w + 63 reach one point into the next word.
                        let any = rows.iter().fold(0, |m, r| m | r[w]);
                        let all = rows.iter().fold(!0, |m, r| m & r[w]);
                        let any_next = rows.iter().fold(0, |m, r| m | r[w + 1]) & 1;
                        let all_next = rows.iter().fold(1, |m, r| m & r[w + 1]) & 1;
                        if (any == 0 && any_next == 0) || (all == !0 && all_next == 1) {
                            continue;
                        }
                        for i in 64 * w..(64 * w + 64).min(nx - 1) {
                            self.cube([i, j, k], inside, &mut tris, &mut quads);
                        }
                    }
                }
                (tris, quads)
            })
            .collect();
        let mut tris = Vec::new();
        let mut quads = Vec::new();
        for (t, q) in per_slab {
            tris.extend(t);
            quads.extend(q);
        }
        (tris, quads)
    }

    fn cube(
        &self,
        at: [usize; 3],
        inside: &Inside,
        tris: &mut Vec<[u64; 3]>,
        quads: &mut Vec<[u64; 4]>,
    ) {
        let corner = |c: u8| {
            [
                at[0] + (c & 1) as usize,
                at[1] + (c >> 1 & 1) as usize,
                at[2] + (c >> 2 & 1) as usize,
            ]
        };
        let mut bits = 0u8;
        for c in 0..8u8 {
            if self.at(inside, corner(c)) {
                bits |= 1 << c;
            }
        }
        if bits == 0 || bits == 0xff {
            return;
        }
        // Corner offsets in half cells, for deciding which way a piece faces.
        let half = |c: u8| {
            [
                (c & 1) as i64 * 2,
                (c >> 1 & 1) as i64 * 2,
                (c >> 2 & 1) as i64 * 2,
            ]
        };
        let mid = |u: u8, v: u8| {
            let (a, b) = (half(u), half(v));
            [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2, (a[2] + b[2]) / 2]
        };
        // The edge between two corners of a tetrahedron: along the path, one corner's bits hold
        // the other's.
        let edge = |u: u8, v: u8| {
            let (lo, hi) = if u & v == u { (u, v) } else { (v, u) };
            (self.key(corner(lo), hi & !lo), mid(u, v))
        };
        for tet in TETS {
            let (ins, outs): (Vec<u8>, Vec<u8>) = tet.iter().partition(|&&c| bits >> c & 1 == 1);
            if ins.is_empty() || outs.is_empty() {
                continue;
            }
            // From the inside corners toward the outside ones, in half cells × corner counts.
            let sum = |cs: &[u8]| cs.iter().fold([0i64; 3], |s, &c| add_i(s, half(c)));
            let toward = sub_i(
                scale_i(sum(&outs), ins.len() as i64),
                scale_i(sum(&ins), outs.len() as i64),
            );
            match (ins.len(), outs.len()) {
                (1, 3) | (3, 1) => {
                    let (apex, rest) = if ins.len() == 1 {
                        (ins[0], &outs)
                    } else {
                        (outs[0], &ins)
                    };
                    let [a, b, c] = [rest[0], rest[1], rest[2]].map(|r| edge(apex, r));
                    let n = cross_i(sub_i(b.1, a.1), sub_i(c.1, a.1));
                    tris.push(if dot_i(n, toward) > 0 {
                        [a.0, b.0, c.0]
                    } else {
                        [a.0, c.0, b.0]
                    });
                }
                _ => {
                    let ([p, q], [r, s]) = ([ins[0], ins[1]], [outs[0], outs[1]]);
                    let corners = [edge(p, r), edge(p, s), edge(q, s), edge(q, r)];
                    let n = cross_i(
                        sub_i(corners[2].1, corners[0].1),
                        sub_i(corners[3].1, corners[1].1),
                    );
                    let mut keys = corners.map(|c| c.0);
                    if dot_i(n, toward) < 0 {
                        keys.reverse();
                    }
                    quads.push(keys);
                }
            }
        }
    }

    /// Where the vertex on grid edge `key` goes: where the edge meets the triangles nearest its
    /// outside end, kept `CLEAR` (at least `CLEAR_MIN` of the edge) from either end; by the inside
    /// end when it meets none (the inside end stands for a wall thinner than a cell, which lies
    /// on its other side).
    fn vertex(
        &self,
        key: u64,
        inside: &Inside,
        points: &[P3],
        triangles: &[[u32; 3]],
        bvh: &Bvh,
    ) -> P3 {
        let d = (key & 7) as u8;
        let g = (key >> 3) as usize;
        let (i, j, k) = (
            g % self.n[0],
            g / self.n[0] % self.n[1],
            g / self.n[0] / self.n[1],
        );
        let from = [i, j, k];
        let to = [
            i + (d & 1) as usize,
            j + (d >> 1 & 1) as usize,
            k + (d >> 2 & 1) as usize,
        ];
        let (outer, inner) = if self.at(inside, from) {
            (to, from)
        } else {
            (from, to)
        };
        let (o, p) = (self.point(outer), self.point(inner));
        let dir = sub(p, o);
        let mut best = f64::INFINITY;
        bvh.segment(o, p, |t| {
            let [a, b, c] = triangles[t as usize].map(|v| points[v as usize]);
            if let Some(s) = segment_hit(o, dir, a, b, c) {
                best = best.min(s);
            }
        });
        let s = if best.is_finite() { best } else { 1.0 };
        let length = exact::length(dir);
        let clear = (CLEAR / length).clamp(CLEAR_MIN, 0.5);
        let s = s.clamp(clear, 1.0 - clear);
        [0, 1, 2].map(|a| o[a] + s * dir[a])
    }
}

/// Where the segment `o` + s·`dir`, s in 0..1, meets triangle `a`, `b`, `c` (allowing for
/// rounding at its edges), as s.
pub(crate) fn segment_hit(o: P3, dir: P3, a: P3, b: P3, c: P3) -> Option<f64> {
    let (e1, e2) = (sub(b, a), sub(c, a));
    let h = cross(dir, e2);
    let det = dot(e1, h);
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let f = 1.0 / det;
    let w = sub(o, a);
    let u = f * dot(w, h);
    let q = cross(w, e1);
    let v = f * dot(dir, q);
    let s = f * dot(e2, q);
    let slack = 1e-9;
    (u >= -slack && v >= -slack && u + v <= 1.0 + slack && (-slack..=1.0 + slack).contains(&s))
        .then_some(s.clamp(0.0, 1.0))
}

/// The sign of orient2d(`u`, `v`, `q`) with `q` moved by an infinitesimal (ε, ε²): never zero for
/// `u` ≠ `v`.
fn side(u: [f64; 2], v: [f64; 2], q: [f64; 2]) -> i32 {
    match orient2d(u, v, q) {
        0 if v[1] != u[1] => {
            if u[1] > v[1] {
                1
            } else {
                -1
            }
        }
        0 => {
            if v[0] > u[0] {
                1
            } else {
                -1
            }
        }
        s => s,
    }
}

type I3 = [i64; 3];

fn add_i(a: I3, b: I3) -> I3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub_i(a: I3, b: I3) -> I3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale_i(a: I3, s: i64) -> I3 {
    a.map(|x| x * s)
}

fn dot_i(a: I3, b: I3) -> i64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross_i(a: I3, b: I3) -> I3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{make_box, stats};
    use crate::ops::concat;
    use crate::repair::{silent, status, tritri};

    fn rebuilt(mesh: &Mesh, cell: f64) -> Mesh {
        silent(|c| remesh(mesh, None, cell, c))
            .unwrap()
            .unwrap()
            .mesh
    }

    /// Closed, oriented, outward and without crossing triangles.
    fn assert_sound(mesh: &Mesh) {
        let s = silent(|c| status::analyze(mesh, c)).unwrap().status;
        assert!(s.closed && s.oriented, "{s:?}");
        assert!(s.volume > 0.0, "{s:?}");
        let crossing = silent(|c| tritri::intersecting_pairs(mesh, c)).unwrap();
        assert!(crossing.is_empty(), "{} crossing pairs", crossing.len());
    }

    fn turned(mut m: Mesh, degrees: f64, by: [f32; 3]) -> Mesh {
        let (s, c) = degrees.to_radians().sin_cos();
        for p in m.positions.as_chunks_mut::<3>().0 {
            let (x, y) = (p[0] as f64, p[1] as f64);
            p[0] = (c * x - s * y) as f32 + by[0];
            p[1] = (s * x + c * y) as f32 + by[1];
            p[2] += by[2];
        }
        m
    }

    #[test]
    fn a_box_comes_back_as_a_box() {
        let b = make_box([0.0; 3], [10.0, 6.0, 4.0]);
        let out = rebuilt(&b, 0.5);
        assert_sound(&out);
        let s = stats(&out);
        assert_eq!(s.shells, 1);
        // The faces stay where they were; the edges and corners are cut by up to a cell.
        assert!((s.volume - 240.0).abs() < 20.0, "{}", s.volume);
        for p in out.positions.as_chunks::<3>().0 {
            assert!(
                p.iter()
                    .zip([10.0, 6.0, 4.0])
                    .all(|(&x, hi)| (-1e-4..=hi + 1e-4).contains(&x)),
                "{p:?}"
            );
        }
    }

    #[test]
    fn overlapping_turned_boxes_become_one_shell() {
        let parts = [
            make_box([-5.0, -1.0, 0.0], [5.0, 1.0, 2.0]),
            turned(
                make_box([-5.0, -1.0, 0.0], [5.0, 1.0, 2.0]),
                37.0,
                [0.3, 0.1, 0.7],
            ),
            turned(
                make_box([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0]),
                11.0,
                [4.0, 2.5, 1.0],
            ),
        ];
        let input = concat(&parts.iter().collect::<Vec<_>>()).unwrap();
        let out = rebuilt(&input, 0.2);
        assert_sound(&out);
        assert_eq!(stats(&out).shells, 1);
    }

    /// No answer for the winding number is right for an open box, but the result is still sound.
    #[test]
    fn an_open_box_still_gives_a_closed_surface() {
        let b = make_box([0.0; 3], [4.0; 3]);
        let mut open = b.clone();
        open.indices.truncate(open.indices.len() - 6);
        let out = rebuilt(&open, 0.25);
        assert_sound(&out);
    }

    #[test]
    fn a_triangle_soup_gives_a_closed_surface() {
        let mut seed = 7u64;
        let mut next = || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((seed >> 11) as f64 / (1u64 << 53) as f64 * 10.0) as f32
        };
        let mut positions = Vec::new();
        for _ in 0..300 * 3 {
            positions.extend([next(), next(), next()]);
        }
        let soup = Mesh {
            indices: (0..positions.len() as u32 / 3).collect(),
            positions,
        };
        let out = rebuilt(&soup, 0.2);
        // Cavities face into themselves (Wrap part surface drops them), so only closed,
        // consistently oriented and free of crossings.
        let edges = crate::repair::topology::Edges::new(&out);
        for e in 0..edges.len() as u32 {
            assert_eq!(edges.degree(e), 2);
            assert!(crate::repair::topology::is_consistent(&out, &edges, e));
        }
        assert!(
            silent(|c| tritri::intersecting_pairs(&out, c))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn only_what_lies_within_the_boxes_is_rebuilt() {
        let parts = [
            make_box([0.0; 3], [2.0; 3]),
            make_box([5.0, 0.0, 0.0], [7.0, 2.0, 2.0]),
            make_box([10.0, 0.0, 0.0], [12.0, 2.0, 2.0]),
        ];
        let all = concat(&parts.iter().collect::<Vec<_>>()).unwrap();
        let boxes = [[[-0.5; 3], [2.5; 3]], [[9.5, -0.5, -0.5], [12.5, 2.5, 2.5]]];
        let out = silent(|c| remesh(&all, Some(&boxes), 0.25, c))
            .unwrap()
            .unwrap()
            .mesh;
        assert_sound(&out);
        assert_eq!(stats(&out).shells, 2);
        let xs = out.positions.as_chunks::<3>().0.iter().map(|p| p[0]);
        assert!(xs.into_iter().all(|x| x < 3.0 || x > 9.0));
    }

    /// A wall a fifth of a cell thick, tilted, is kept whole, its faces where they were.
    #[test]
    fn a_wall_thinner_than_a_cell_stays_whole() {
        let wall = turned(
            make_box([-10.0, -5.0, 0.0], [10.0, 5.0, 0.1]),
            20.0,
            [0.0; 3],
        );
        let mut wall = wall;
        // Tilt it about x by about 30 degrees.
        for p in wall.positions.as_chunks_mut::<3>().0 {
            let (y, z) = (p[1] as f64, p[2] as f64);
            let (s, c) = 0.5f64.sin_cos();
            p[1] = (c * y - s * z) as f32;
            p[2] = (s * y + c * z) as f32;
        }
        let out = rebuilt(&wall, 0.5);
        assert_sound(&out);
        assert_eq!(stats(&out).shells, 1);
        let wall_area = 2.0 * 20.0 * 10.0;
        let s = stats(&out);
        assert!(
            s.area > 0.8 * wall_area && s.area < 1.3 * wall_area,
            "{}",
            s.area
        );
    }
}
