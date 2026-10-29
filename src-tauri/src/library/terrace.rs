//! Reliefs in bands: QR codes, pictures and base plates. A grid of samples is split into triangles;
//! within a triangle the value is linear, so each band of values is a convex piece of it and the
//! edges between bands are straight segments, which the neighbouring triangles share exactly.
//! Every band stands as a flat-topped column from the base up to its height (or is left out, a
//! hole), with vertical walls where bands meet and round the outside. So the result is closed,
//! oriented and free of crossings by construction: faces are flat or vertical, walls follow
//! disjoint band edges, and every point on a vertical line is shared by all the faces that meet
//! there.

use std::collections::HashMap;

use super::build::{Builder, P2};
use crate::mesh::Mesh;

/// A point of the subdivision: a grid point, or where threshold `t` crosses the grid edge between
/// grid points `g` and `h` (stored with g < h).
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum Pt {
    Corner(usize),
    Cross(usize, usize, usize),
}

/// A point at a height: the top of band `Some(k)`, or the base (None).
type Key = (Pt, Option<usize>);

struct Terrace<'a> {
    xs: &'a [f64],
    ys: &'a [f64],
    value: &'a [f64],
    thresholds: &'a [f64],
    heights: &'a [Option<f64>],
    base: f64,
    b: Builder,
    ids: HashMap<Key, u32>,
}

impl Terrace<'_> {
    fn band(&self, g: usize) -> usize {
        let v = self.value[g];
        self.thresholds.iter().filter(|&&t| v > t).count()
    }

    fn at(&self, g: usize) -> P2 {
        let nx = self.xs.len();
        [self.xs[g % nx], self.ys[g / nx]]
    }

    fn place(&self, pt: Pt) -> P2 {
        match pt {
            Pt::Corner(g) => self.at(g),
            Pt::Cross(g, h, t) => {
                let (u, w) = (self.value[g], self.value[h]);
                let s = (self.thresholds[t] - u) / (w - u);
                let (p, q) = (self.at(g), self.at(h));
                [p[0] + s * (q[0] - p[0]), p[1] + s * (q[1] - p[1])]
            }
        }
    }

    /// The height of a band's top, or the base for None or a band left out.
    fn level(&self, k: Option<usize>) -> f64 {
        k.and_then(|k| self.heights[k]).unwrap_or(self.base)
    }

    /// The band's slot for keys: Some(k) when it stands, None (the base) when it is left out.
    fn slot(&self, k: usize) -> Option<usize> {
        self.heights[k].map(|_| k)
    }

    fn id(&mut self, pt: Pt, k: Option<usize>) -> u32 {
        if let Some(&id) = self.ids.get(&(pt, k)) {
            return id;
        }
        let p = self.place(pt);
        let z = self.level(k);
        let id = self.b.point([p[0], p[1], z]);
        self.ids.insert((pt, k), id);
        id
    }

    /// The crossing of threshold `t` on the edge from `g` to `h`, keyed the same from either end.
    fn cross(g: usize, h: usize, t: usize) -> Pt {
        Pt::Cross(g.min(h), g.max(h), t)
    }

    /// The points along the edge from `g` to `h`: g, the crossings in order from g, then h.
    fn along(&self, g: usize, h: usize) -> Vec<Pt> {
        let (bg, bh) = (self.band(g), self.band(h));
        let mut out = vec![Pt::Corner(g)];
        if bg < bh {
            out.extend((bg..bh).map(|t| Self::cross(g, h, t)));
        } else if bg > bh {
            out.extend((bh..bg).rev().map(|t| Self::cross(g, h, t)));
        }
        out.push(Pt::Corner(h));
        out
    }

    /// The bands a point belongs to: a corner one, a crossing of t the two either side.
    fn bands_of(&self, pt: Pt) -> (usize, usize) {
        match pt {
            Pt::Corner(g) => (self.band(g), self.band(g)),
            Pt::Cross(_, _, t) => (t, t + 1),
        }
    }

    /// The band of the stretch between two neighbouring points along an edge.
    fn between(&self, p: Pt, q: Pt) -> usize {
        match (p, q) {
            (Pt::Corner(g), _) => self.band(g),
            (_, Pt::Corner(h)) => self.band(h),
            (Pt::Cross(_, _, s), Pt::Cross(_, _, u)) => s.max(u),
        }
    }

    /// The heights on the vertical line through `pt`, from the base up: the base and the top of
    /// each standing band it belongs to.
    fn column(&mut self, pt: Pt) -> Vec<(f64, u32)> {
        let (x, y) = self.bands_of(pt);
        let mut slots = vec![None];
        for k in [x, y] {
            if let Some(s) = self.slot(k)
                && !slots.contains(&Some(s))
            {
                slots.push(Some(s));
            }
        }
        let mut out: Vec<(f64, u32)> = slots
            .into_iter()
            .map(|s| (self.level(s), self.id(pt, s)))
            .collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.dedup_by(|a, b| a.0 == b.0);
        out
    }

    /// A vertical wall from `p` to `q` facing right of p → q, between heights `lo` and `hi`, with
    /// every point of the two vertical lines between them, the two sides zipped by height.
    fn wall(&mut self, p: Pt, q: Pt, lo: f64, hi: f64) {
        let side = |t: &mut Self, pt: Pt| -> Vec<(f64, u32)> {
            t.column(pt)
                .into_iter()
                .filter(|&(z, _)| z >= lo && z <= hi)
                .collect()
        };
        let left = side(self, p);
        let right = side(self, q);
        let (mut i, mut j) = (0, 0);
        // The quad (p lo, q lo, q hi, p hi) faces right of p → q; between the two chains each step
        // up one of them adds one triangle.
        while i + 1 < left.len() || j + 1 < right.len() {
            let up_right =
                i + 1 >= left.len() || (j + 1 < right.len() && right[j + 1].0 <= left[i + 1].0);
            if up_right {
                self.b.triangle(left[i].1, right[j].1, right[j + 1].1);
                j += 1;
            } else {
                self.b.triangle(left[i].1, right[j].1, left[i + 1].1);
                i += 1;
            }
        }
    }
}

/// A relief over a grid of samples (`value[j * xs.len() + i]` at (xs[i], ys[j]), both increasing):
/// band k holds the values between `thresholds[k - 1]` and `thresholds[k]` (band 0 below the first
/// threshold, the last band above the last) and stands from `base` up to `heights[k]`, or is left
/// out (a hole) when that is None. The standing bands' heights must differ from each other and lie
/// above the base, and no sample may equal a threshold.
pub(crate) fn terrace(
    xs: &[f64],
    ys: &[f64],
    value: &[f64],
    thresholds: &[f64],
    heights: &[Option<f64>],
    base: f64,
) -> Result<Mesh, String> {
    assert_eq!(heights.len(), thresholds.len() + 1);
    assert_eq!(value.len(), xs.len() * ys.len());
    let (nx, ny) = (xs.len(), ys.len());
    let mut t = Terrace {
        xs,
        ys,
        value,
        thresholds,
        heights,
        base,
        b: Builder::default(),
        ids: HashMap::new(),
    };
    for j in 0..ny - 1 {
        for i in 0..nx - 1 {
            let g = j * nx + i;
            // Two triangles per cell, each anticlockwise seen from above.
            for tri in [[g, g + 1, g + nx + 1], [g, g + nx + 1, g + nx]] {
                triangle_pieces(&mut t, tri);
            }
        }
    }
    // Walls round the outside, anticlockwise seen from above, so the outside is on the right.
    let mut border: Vec<usize> = Vec::new();
    border.extend(0..nx);
    border.extend((1..ny).map(|j| j * nx + nx - 1));
    border.extend((0..nx - 1).rev().map(|i| (ny - 1) * nx + i));
    border.extend((1..ny - 1).rev().map(|j| j * nx));
    let n = border.len();
    for e in 0..n {
        let along = t.along(border[e], border[(e + 1) % n]);
        for pair in along.windows(2) {
            let k = t.between(pair[0], pair[1]);
            if t.heights[k].is_some() {
                let top = t.level(Some(k));
                t.wall(pair[0], pair[1], t.base, top);
            }
        }
    }
    t.b.finish_in_place()
}

/// One triangle of the grid: the top and bottom face of each standing band's piece, and the walls
/// along the band edges crossing it.
fn triangle_pieces(t: &mut Terrace, tri: [usize; 3]) {
    // The outline anticlockwise: corners and crossings in order.
    let mut outline: Vec<Pt> = Vec::new();
    for e in 0..3 {
        let along = t.along(tri[e], tri[(e + 1) % 3]);
        outline.extend_from_slice(&along[..along.len() - 1]);
    }
    let bands: Vec<usize> = tri.iter().map(|&g| t.band(g)).collect();
    let (lo, hi) = (*bands.iter().min().unwrap(), *bands.iter().max().unwrap());
    for k in lo..=hi {
        if t.heights[k].is_none() {
            continue;
        }
        let piece: Vec<Pt> = outline
            .iter()
            .copied()
            .filter(|&pt| {
                let (x, y) = t.bands_of(pt);
                x == k || y == k
            })
            .collect();
        if piece.len() < 3 {
            continue;
        }
        let top: Vec<u32> = piece.iter().map(|&pt| t.id(pt, Some(k))).collect();
        let bottom: Vec<u32> = piece.iter().map(|&pt| t.id(pt, None)).collect();
        for m in 1..top.len() - 1 {
            t.b.triangle(top[0], top[m], top[m + 1]);
            t.b.triangle(bottom[0], bottom[m + 1], bottom[m]);
        }
    }
    // A wall along each band edge, facing the lower of the two bands.
    for th in lo..hi {
        let ends: Vec<Pt> = outline
            .iter()
            .copied()
            .filter(|pt| matches!(pt, Pt::Cross(_, _, u) if *u == th))
            .collect();
        let [p, q] = ends[..] else {
            unreachable!("a band edge crosses a triangle's outline exactly twice");
        };
        let (below, above) = (t.level(t.slot(th)), t.level(t.slot(th + 1)));
        if below == above {
            continue;
        }
        // Which side of p → q the values above the threshold lie on: a corner of the band above.
        let high = tri
            .iter()
            .copied()
            .find(|&g| t.band(g) > th)
            .expect("the triangle spans the threshold");
        let (pp, qp, hp) = (t.place(p), t.place(q), t.at(high));
        let right = [qp[1] - pp[1], pp[0] - qp[0]];
        let above_on_right = right[0] * (hp[0] - pp[0]) + right[1] * (hp[1] - pp[1]) > 0.0;
        // The wall faces right of its direction; it must face the lower band.
        let lower_on_right = above_on_right == (above < below);
        let (from, to) = if lower_on_right { (p, q) } else { (q, p) };
        t.wall(from, to, below.min(above), below.max(above));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::tests::assert_clean;

    fn grid(n: usize) -> Vec<f64> {
        (0..n).map(|i| i as f64).collect()
    }

    #[test]
    fn a_checkerboard_of_two_bands_is_closed_and_has_the_volume_of_its_columns() {
        // 4 × 4 samples in a checkerboard: every inner grid point is a pinch of the two bands.
        let xs = grid(4);
        let value: Vec<f64> = (0..16).map(|g| ((g % 4 + g / 4) % 2) as f64).collect();
        let mesh = terrace(&xs, &xs, &value, &[0.5], &[Some(1.0), Some(2.0)], 0.0).unwrap();
        assert_clean(&mesh, "checkerboard");
        let v = crate::mesh::stats(&mesh).volume;
        // With every cell split along the same diagonal, the band edges cut a corner triangle of
        // area 1/8 off each half cell: the 5 cells whose lower-left sample is 0 keep 1/4 at the
        // upper height, the 4 others 3/4. So 4.25 of the 9 stands at 2 and 4.75 at 1.
        assert!((v - 13.25).abs() < 1e-9, "{v}");
    }

    #[test]
    fn holes_and_several_bands_stay_closed() {
        let xs = grid(7);
        let value: Vec<f64> = (0..49)
            .map(|g| ((g * 37 + g / 7 * 11) % 5) as f64)
            .collect();
        let mesh = terrace(
            &xs,
            &xs,
            &value,
            &[0.5, 1.5, 2.5, 3.5],
            &[None, Some(1.0), Some(3.0), Some(2.0), Some(4.0)],
            -0.5,
        )
        .unwrap();
        assert_clean(&mesh, "bands with holes");
    }
}
