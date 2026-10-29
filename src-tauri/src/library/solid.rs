//! Solids the library cannot write face by face: the surface where a function changes sign
//! (gyroid, fractal, Klein bottle), the outer faces of a set of cubes (Menger sponge), and
//! overlapping closed solids joined into one (the samples, a screw's slotted head, the test parts).

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use rayon::prelude::*;

use super::build::{Builder, P};
use crate::mesh::Mesh;
use crate::repair::levelset::{Grid, Inside, surface};
use crate::repair::{Control, RepairError, RepairResult, split_edges, wrap};

/// Grid points beyond this are refused before any work: a field this fine would take minutes.
pub(crate) const MAX_GRID_POINTS: usize = 120_000_000;

/// How many grid points a field over `lo..hi` at `cell` samples.
pub(crate) fn grid_points(lo: P, hi: P, cell: f64) -> usize {
    (0..3)
        .map(|k| ((hi[k] - lo[k]) / cell).ceil() as usize + 4)
        .product()
}

/// The surface between the points where `f` is negative (inside) and the others, sampled on a grid
/// of `cell` mm over the box `lo..hi`, which must hold all of the inside (the grid adds a clear
/// border). The surface is closed, oriented and free of crossings by construction (src/repair/
/// levelset.rs): each vertex lies on a grid edge between an inside and an outside point, where the
/// function crosses zero along it. None when nothing is inside.
pub(crate) fn field(
    lo: P,
    hi: P,
    cell: f64,
    f: impl Fn(P) -> f64 + Sync,
    control: &mut Control,
) -> RepairResult<Option<Mesh>> {
    if grid_points(lo, hi, cell) > MAX_GRID_POINTS {
        return Err(RepairError::Failed(
            "That is too fine: make the accuracy coarser or the part smaller.".into(),
        ));
    }
    let grid = Grid::new(&[lo, hi], cell);
    let [nx, ny, nz] = grid.n;
    let cancel = control.cancel_flag();
    // Sample every point once; the inside map and the vertex placement both read the samples.
    let samples: Vec<Vec<f32>> = (0..nz)
        .into_par_iter()
        .map(|k| {
            let mut slab = Vec::with_capacity(nx * ny);
            if cancel.load(Ordering::Relaxed) {
                return slab;
            }
            for j in 0..ny {
                for i in 0..nx {
                    slab.push(f(grid.point([i, j, k])) as f32);
                }
            }
            slab
        })
        .collect();
    control.check()?;
    control.progress(0.5);
    let slabs: Vec<Vec<u64>> = samples
        .par_iter()
        .map(|slab| {
            let mut bits = vec![0u64; ny * grid.row_words];
            for j in 0..ny {
                for i in 0..nx {
                    // NaN (a function that cannot be evaluated there) counts as outside.
                    if slab[j * nx + i] < 0.0 {
                        bits[j * grid.row_words + i / 64] |= 1 << (i % 64);
                    }
                }
            }
            bits
        })
        .collect();
    let inside = Inside { slabs };
    let value = |[i, j, k]: [usize; 3]| samples[k][j * nx + i] as f64;
    let place = |key: u64| {
        let [a, b] = grid.ends(key);
        let (fa, fb) = (value(a), value(b));
        // Where the function crosses zero along the edge, kept strictly between its ends.
        let s = if fa.is_finite() && fb.is_finite() && fa != fb {
            (fa / (fa - fb)).clamp(0.02, 0.98)
        } else {
            0.5
        };
        let (pa, pb) = (grid.point(a), grid.point(b));
        [0, 1, 2].map(|k| pa[k] + s * (pb[k] - pa[k]))
    };
    surface(&grid, &inside, &mut control.sub(0.5, 1.0), place)
}

/// The outer faces of the cubes of an `n[0]` × `n[1]` × `n[2]` grid of `cell` mm whose `solid`
/// says so, sharing corners. Edges four faces meet at (cubes touching along an edge only) are
/// split, so the result is closed and manifold.
pub(crate) fn voxels(
    n: [usize; 3],
    cell: f64,
    solid: impl Fn(usize, usize, usize) -> bool,
    control: &mut Control,
) -> RepairResult<Mesh> {
    let mut b = Builder::default();
    let mut corner: HashMap<[usize; 3], u32> = HashMap::new();
    let mut id = |b: &mut Builder, c: [usize; 3]| -> u32 {
        *corner
            .entry(c)
            .or_insert_with(|| b.point(c.map(|v| v as f64 * cell)))
    };
    let filled = |i: isize, j: isize, k: isize| {
        i >= 0
            && j >= 0
            && k >= 0
            && (i as usize) < n[0]
            && (j as usize) < n[1]
            && (k as usize) < n[2]
            && solid(i as usize, j as usize, k as usize)
    };
    for k in 0..n[2] {
        control.check()?;
        for j in 0..n[1] {
            for i in 0..n[0] {
                if !solid(i, j, k) {
                    continue;
                }
                let (si, sj, sk) = (i as isize, j as isize, k as isize);
                // Each face: the neighbour across it, and its corners anticlockwise seen from
                // outside.
                let faces: [([isize; 3], [[usize; 3]; 4]); 6] = [
                    ([-1, 0, 0], [[0, 0, 0], [0, 0, 1], [0, 1, 1], [0, 1, 0]]),
                    ([1, 0, 0], [[1, 0, 0], [1, 1, 0], [1, 1, 1], [1, 0, 1]]),
                    ([0, -1, 0], [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]]),
                    ([0, 1, 0], [[0, 1, 0], [0, 1, 1], [1, 1, 1], [1, 1, 0]]),
                    ([0, 0, -1], [[0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]]),
                    ([0, 0, 1], [[0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]]),
                ];
                for (d, corners) in faces {
                    if filled(si + d[0], sj + d[1], sk + d[2]) {
                        continue;
                    }
                    let ids = corners.map(|c| id(&mut b, [i + c[0], j + c[1], k + c[2]]));
                    b.quad(ids[0], ids[1], ids[2], ids[3]);
                }
            }
        }
        control.progress(0.8 * (k + 1) as f64 / n[2] as f64);
    }
    let mesh = b.raw().map_err(RepairError::Failed)?;
    let split = split_edges::split_non_manifold(&mesh, &[], &mut control.sub(0.8, 1.0))?;
    Ok(split.mesh)
}

/// Overlapping closed, outward-facing solids joined into one by Wrap part surface (their union;
/// vertices land on its 0.002 mm grid). Parts that touch only should overlap a little instead.
pub(crate) fn union(parts: &[Mesh], control: &mut Control) -> RepairResult<Mesh> {
    let mut b = Builder::default();
    for p in parts {
        b.append(p);
    }
    let joined = b.raw().map_err(RepairError::Failed)?;
    let welded = crate::mesh::weld_indexed(&joined.positions, &joined.indices)
        .map_err(RepairError::Failed)?;
    Ok(wrap::run(&welded, &[], control)?.mesh)
}
