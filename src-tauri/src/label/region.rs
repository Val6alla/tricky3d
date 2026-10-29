//! The flat shape of a label: the area its text or picture covers, as polygons with holes in the
//! label's plane. Glyph and picture outlines may overlap (an italic letter reaching over the next)
//! or touch at a point (two pixels meeting at a corner); `Region::from_contours` joins them into
//! polygons that do neither, so the walls built round them never share an edge.

use spade::handles::FixedVertexHandle;
use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};

use crate::library::Polygon;

pub(crate) type P2 = [f64; 2];

/// How far a point where two filled areas meet is moved into each of them, in the units of the
/// contours, so the two outlines no longer touch.
const PINCH: f64 = 1e-3;

/// Polygons with holes: each outline anticlockwise, each hole clockwise, none touching another.
#[derive(Clone, Debug, Default)]
pub(crate) struct Region {
    pub polygons: Vec<Polygon>,
    index: Bands,
}

impl Region {
    /// The area inside `contours` by the non-zero rule: a point is inside when the contours wind
    /// round it on balance, so overlapping glyphs join and a counter-wound contour cuts a hole.
    /// Empty contours and contours of fewer than three distinct points are left out.
    pub(crate) fn from_contours(contours: &[Vec<P2>]) -> Result<Region, String> {
        let contours: Vec<Vec<P2>> = contours.iter().filter_map(|c| cleaned(c)).collect();
        if contours.is_empty() {
            return Ok(Region::default());
        }
        let winding = Bands::new(contours.iter().map(|c| c.as_slice()));
        let mut cdt: ConstrainedDelaunayTriangulation<Point2<f64>> =
            ConstrainedDelaunayTriangulation::new();
        let bad = || "The outline could not be read.".to_string();
        for contour in &contours {
            let handles: Vec<FixedVertexHandle> = contour
                .iter()
                .map(|p| cdt.insert(Point2::new(p[0], p[1])).map_err(|_| bad()))
                .collect::<Result<_, _>>()?;
            for k in 0..handles.len() {
                let (a, b) = (handles[k], handles[(k + 1) % handles.len()]);
                if a != b {
                    cdt.add_constraint_and_split(a, b, |p| p);
                }
            }
        }
        // Each triangle is inside or outside as a whole: no contour passes through one. Its
        // winding number is read at its centre, but only where the triangle is not a sliver: the
        // centre of a sliver lies so close to its sides that rounding can put it on the wrong
        // one. Slivers take the number of a neighbour across a side no contour runs along.
        let mut number: Vec<Option<i32>> = vec![None; cdt.num_all_faces()];
        let mut queue = std::collections::VecDeque::new();
        let centre_of = |face: spade::handles::FaceHandle<
            '_,
            spade::handles::InnerTag,
            Point2<f64>,
            (),
            spade::CdtEdge<()>,
            (),
        >| {
            let [a, b, c] = face.positions();
            [(a.x + b.x + c.x) / 3.0, (a.y + b.y + c.y) / 3.0]
        };
        for face in cdt.inner_faces() {
            let [a, b, c] = face.positions();
            let twice_area = ((b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)).abs();
            let longest = [(a, b), (b, c), (c, a)]
                .iter()
                .map(|(p, q)| (q.x - p.x).powi(2) + (q.y - p.y).powi(2))
                .fold(0.0, f64::max);
            if twice_area > 1e-6 * longest {
                number[face.fix().index()] = Some(winding.winding(centre_of(face)));
                queue.push_back(face.fix());
            }
        }
        while let Some(f) = queue.pop_front() {
            let w = number[f.index()];
            for e in cdt.face(f).adjacent_edges() {
                if cdt.is_constraint_edge(e.as_undirected().fix()) {
                    continue;
                }
                if let Some(n) = e.rev().face().as_inner()
                    && number[n.fix().index()].is_none()
                {
                    number[n.fix().index()] = w;
                    queue.push_back(n.fix());
                }
            }
        }
        let mut filled = vec![false; cdt.num_all_faces()];
        for face in cdt.inner_faces() {
            let w = number[face.fix().index()].unwrap_or_else(|| winding.winding(centre_of(face)));
            filled[face.fix().index()] = w != 0;
        }
        // The edges with a filled face on their left and an empty one (or none) on their right.
        let count = cdt.num_vertices();
        let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); count];
        let mut sides: Vec<(usize, usize)> = Vec::new();
        for e in cdt.directed_edges() {
            let left = e.face().as_inner().is_some_and(|f| filled[f.fix().index()]);
            let right = e
                .rev()
                .face()
                .as_inner()
                .is_some_and(|f| filled[f.fix().index()]);
            if left && !right {
                outgoing[e.from().fix().index()].push(sides.len());
                sides.push((e.from().fix().index(), e.to().fix().index()));
            }
        }
        let at = |i: usize| {
            let p = cdt.vertex(FixedVertexHandle::from_index(i)).position();
            [p.x, p.y]
        };
        let loops = chain(&sides, &outgoing, at);
        Ok(Region::from_loops(separated(loops)))
    }

    /// Polygons from loops that neither cross nor touch: anticlockwise ones are outlines, each
    /// clockwise one a hole in the smallest outline round it.
    pub(crate) fn from_loops(loops: Vec<Vec<P2>>) -> Region {
        let (mut outers, holes): (Vec<Vec<P2>>, Vec<Vec<P2>>) =
            loops.into_iter().partition(|lp| area2(lp) > 0.0);
        outers.sort_by(|a, b| area2(a).total_cmp(&area2(b)));
        let mut owned: Vec<Vec<Vec<P2>>> = vec![Vec::new(); outers.len()];
        for hole in holes {
            if let Some(i) = outers.iter().position(|o| inside_loop(o, hole[0])) {
                owned[i].push(hole);
            }
        }
        let polygons: Vec<Polygon> = outers
            .into_iter()
            .zip(owned)
            .map(|(outer, holes)| Polygon::new(outer, holes))
            .filter(|p| p.outer.len() >= 3)
            .collect();
        let index = Bands::new(
            polygons
                .iter()
                .flat_map(|p| p.loops().map(|l| l.as_slice())),
        );
        Region { polygons, index }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.polygons.is_empty()
    }

    /// Whether `p` is inside (even-odd over every loop; the loops neither cross nor touch).
    pub(crate) fn contains(&self, p: P2) -> bool {
        self.index.winding(p) != 0
    }

    /// Every loop, polygon by polygon, outline before holes: the order region vertices are
    /// numbered in.
    pub(crate) fn loops(&self) -> impl Iterator<Item = &Vec<P2>> {
        self.polygons.iter().flat_map(|p| p.loops())
    }

    /// The smallest box round the region; None when it is empty.
    pub(crate) fn bounds(&self) -> Option<(P2, P2)> {
        let mut points = self.loops().flatten().peekable();
        points.peek()?;
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for p in points {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        Some((lo, hi))
    }

    /// Area in square units.
    #[cfg(test)]
    pub(crate) fn area(&self) -> f64 {
        self.loops().map(|l| area2(l)).sum::<f64>() / 2.0
    }

    /// The region with every point moved by `f`. `reflect` says that `f` is a reflection, which
    /// turns every loop round; they are turned back so outlines stay anticlockwise.
    pub(crate) fn mapped(&self, f: impl Fn(P2) -> P2, reflect: bool) -> Region {
        let loops = self
            .loops()
            .map(|l| {
                let mut lp: Vec<P2> = l.iter().map(|&p| f(p)).collect();
                if reflect {
                    lp.reverse();
                }
                lp
            })
            .collect();
        Region::from_loops(loops)
    }

    /// The region without points closer than `shortest` to the one kept before them, so no side
    /// is shorter than that. Loops left with fewer than three points go.
    pub(crate) fn without_short_sides(&self, shortest: f64) -> Region {
        let near = |p: P2, q: P2| (p[0] - q[0]).hypot(p[1] - q[1]) < shortest;
        let loops = self
            .loops()
            .filter_map(|l| {
                let mut kept: Vec<P2> = Vec::with_capacity(l.len());
                for &p in l {
                    if kept.last().is_none_or(|&q| !near(p, q)) {
                        kept.push(p);
                    }
                }
                while kept.len() > 3 && near(kept[0], kept[kept.len() - 1]) {
                    kept.pop();
                }
                (kept.len() >= 3 && area2(&kept) != 0.0).then_some(kept)
            })
            .collect();
        Region::from_loops(loops)
    }

    /// Triangles over the region's points, numbered as `loops` numbers them, anticlockwise.
    pub(crate) fn triangles(&self) -> Result<Vec<[usize; 3]>, String> {
        let mut out = Vec::new();
        let mut base = 0;
        for poly in &self.polygons {
            out.extend(poly.triangulate()?.into_iter().map(|t| t.map(|i| base + i)));
            base += poly.loops().map(|l| l.len()).sum::<usize>();
        }
        Ok(out)
    }
}

/// Twice the signed area of a loop (positive anticlockwise).
pub(crate) fn area2(lp: &[P2]) -> f64 {
    let n = lp.len();
    (0..n)
        .map(|k| {
            let (a, b) = (lp[k], lp[(k + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

/// The contour without repeated points (also first against last); None when fewer than three
/// distinct points or no area are left.
fn cleaned(contour: &[P2]) -> Option<Vec<P2>> {
    let mut c: Vec<P2> = contour.to_vec();
    c.dedup();
    while c.len() > 1 && c[0] == c[c.len() - 1] {
        c.pop();
    }
    (c.len() >= 3 && area2(&c) != 0.0).then_some(c)
}

/// Whether `p` is inside the loop (even-odd).
fn inside_loop(lp: &[P2], p: P2) -> bool {
    let mut inside = false;
    let n = lp.len();
    for k in 0..n {
        let (a, b) = (lp[k], lp[(k + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0])
        {
            inside = !inside;
        }
    }
    inside
}

/// The boundary sides joined into loops. Where two filled areas meet at a point, a vertex has
/// two sides leaving it; the one turning furthest left from the side that arrived is taken, so
/// each area keeps its own loop.
fn chain(
    sides: &[(usize, usize)],
    outgoing: &[Vec<usize>],
    at: impl Fn(usize) -> P2,
) -> Vec<Vec<P2>> {
    let mut used = vec![false; sides.len()];
    let mut loops = Vec::new();
    for start in 0..sides.len() {
        if used[start] {
            continue;
        }
        let mut lp = Vec::new();
        let mut side = start;
        loop {
            used[side] = true;
            let (from, to) = sides[side];
            lp.push(at(from));
            let (a, b) = (at(from), at(to));
            let arrive = (b[1] - a[1]).atan2(b[0] - a[0]);
            let next = outgoing[to]
                .iter()
                .copied()
                .filter(|&s| !used[s] || s == start)
                .max_by(|&x, &y| {
                    turn(arrive, &at, sides[x]).total_cmp(&turn(arrive, &at, sides[y]))
                });
            match next {
                Some(s) if s != start => side = s,
                _ => break,
            }
        }
        if lp.len() >= 3 {
            loops.push(lp);
        }
    }
    loops
}

/// The left turn (radians, −π..π) from direction `arrive` onto `side`.
fn turn(arrive: f64, at: &impl Fn(usize) -> P2, side: (usize, usize)) -> f64 {
    let (a, b) = (at(side.0), at(side.1));
    let mut d = (b[1] - a[1]).atan2(b[0] - a[0]) - arrive;
    while d > std::f64::consts::PI {
        d -= std::f64::consts::TAU;
    }
    while d <= -std::f64::consts::PI {
        d += std::f64::consts::TAU;
    }
    d
}

/// The loops with every point that two loops (or one loop twice) share moved a little into the
/// filled side at each visit, so no two loops touch.
fn separated(mut loops: Vec<Vec<P2>>) -> Vec<Vec<P2>> {
    use std::collections::HashMap;
    let mut visits: HashMap<(u64, u64), usize> = HashMap::new();
    for p in loops.iter().flatten() {
        *visits.entry((p[0].to_bits(), p[1].to_bits())).or_default() += 1;
    }
    if visits.values().all(|&n| n == 1) {
        return loops;
    }

    for lp in &mut loops {
        let n = lp.len();
        let original = lp.clone();
        for k in 0..n {
            let p = original[k];
            if visits[&(p[0].to_bits(), p[1].to_bits())] < 2 {
                continue;
            }
            // The filled side is on the left of both sides at the point: move along the bisector
            // of their left normals.
            let (prev, next) = (original[(k + n - 1) % n], original[(k + 1) % n]);
            let left = |a: P2, b: P2| {
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let l = dx.hypot(dy).max(f64::MIN_POSITIVE);
                [-dy / l, dx / l]
            };
            let (l1, l2) = (left(prev, p), left(p, next));
            let mut d = [l1[0] + l2[0], l1[1] + l2[1]];
            let len = d[0].hypot(d[1]);
            if len < 1e-9 {
                d = l1;
            } else {
                d = [d[0] / len, d[1] / len];
            }
            lp[k] = [p[0] + PINCH * d[0], p[1] + PINCH * d[1]];
        }
    }
    loops
}

/// The loops' sides bucketed by height, for winding numbers at many points.
#[derive(Clone, Debug, Default)]
struct Bands {
    sides: Vec<(P2, P2)>,
    lo: f64,
    step: f64,
    bands: Vec<Vec<u32>>,
}

impl Bands {
    fn new<'a>(loops: impl Iterator<Item = &'a [P2]>) -> Bands {
        let mut sides = Vec::new();
        for lp in loops {
            let n = lp.len();
            for k in 0..n {
                sides.push((lp[k], lp[(k + 1) % n]));
            }
        }
        if sides.is_empty() {
            return Bands::default();
        }
        let lo = sides
            .iter()
            .map(|s| s.0[1].min(s.1[1]))
            .fold(f64::INFINITY, f64::min);
        let hi = sides
            .iter()
            .map(|s| s.0[1].max(s.1[1]))
            .fold(f64::NEG_INFINITY, f64::max);
        let count = (sides.len() / 8).clamp(1, 4096);
        let step = ((hi - lo) / count as f64).max(f64::MIN_POSITIVE);
        let mut bands = vec![Vec::new(); count];
        for (i, s) in sides.iter().enumerate() {
            let (a, b) = (s.0[1].min(s.1[1]), s.0[1].max(s.1[1]));
            let first = (((a - lo) / step) as usize).min(count - 1);
            let last = (((b - lo) / step) as usize).min(count - 1);
            for band in &mut bands[first..=last] {
                band.push(i as u32);
            }
        }
        Bands {
            sides,
            lo,
            step,
            bands,
        }
    }

    /// How often the loops wind round `p` anticlockwise.
    fn winding(&self, p: P2) -> i32 {
        if self.bands.is_empty() {
            return 0;
        }
        let k = (p[1] - self.lo) / self.step;
        if !(0.0..=self.bands.len() as f64).contains(&k) {
            return 0;
        }
        let band = &self.bands[(k as usize).min(self.bands.len() - 1)];
        let mut w = 0;
        for &i in band {
            let (a, b) = self.sides[i as usize];
            let side = (b[0] - a[0]) * (p[1] - a[1]) - (p[0] - a[0]) * (b[1] - a[1]);
            if a[1] <= p[1] && b[1] > p[1] && side > 0.0 {
                w += 1;
            } else if b[1] <= p[1] && a[1] > p[1] && side < 0.0 {
                w -= 1;
            }
        }
        w
    }
}
