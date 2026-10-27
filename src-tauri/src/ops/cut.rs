//! Cut by plane for parts manifold-3d cannot cut (open or non-manifold meshes, or Close cut
//! surfaces off): split the triangles that cross the plane, sort every triangle to a side, and
//! optionally fill the closed loops of the cut outline.
//!
//! Watertight by construction: a vertex gets one side for every triangle that uses it, a crossing
//! edge gets one intersection vertex shared by both of its triangles, and the caps reuse the
//! outline's own vertex ids. On a closed input both pieces are closed, and their volumes add up to
//! the input's: the caps cancel exactly, and the only loss is each intersection vertex rounding
//! to f32 a little off its triangles' planes (about 1e-9 of the volume).
//!
//! The parts that come here are often the awkward ones: several shells in one part (a Merge parts
//! result, a multi-body STL) that overlap or sit inside each other, parts wound inside out or
//! inconsistently, outlines with long straight runs. `fill` says how each is handled and what it
//! costs.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};
use std::iter::once;

use super::compact;
use crate::mesh::{Mesh, cross, dot, sub};

/// The plane dot(normal, p) == offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    /// Unit length.
    pub normal: [f64; 3],
    pub offset: f64,
}

#[derive(Debug, Default)]
pub struct CutPieces {
    /// dot(normal, p) <= offset, "cut 1". None when that side has no triangles.
    pub below: Option<Mesh>,
    /// dot(normal, p) >= offset, "cut 2".
    pub above: Option<Mesh>,
    /// Capping was asked for and some cut edge got no cap: an outline chain that does not close
    /// (an open part), or a loop that could not be filled. That hole stays open.
    pub open_outline: bool,
}

/// Two vertex ids: directed as a triangle runs it, or (lower, higher) as a key.
type Edge = (u32, u32);

const BELOW: i8 = -1;
const ON: i8 = 0;
const ABOVE: i8 = 1;

/// Hashes the vertex ids and edges the maps here are keyed by. The std default, SipHash, cost more
/// than the rest of a cut; these keys are the mesh's own numbering, so a multiply-mix is enough.
#[derive(Default)]
struct IdHasher(u64);

impl Hasher for IdHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(b as u64);
        }
    }

    fn write_u32(&mut self, n: u32) {
        self.write_u64(n as u64);
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(32) ^ n).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }

    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }

    fn finish(&self) -> u64 {
        self.0 ^ (self.0 >> 29)
    }
}

type IdMap<K, V> = HashMap<K, V, BuildHasherDefault<IdHasher>>;
type IdSet<K> = HashSet<K, BuildHasherDefault<IdHasher>>;

/// Cuts `mesh` (world space) by `plane`. With `cap`, each side's closed outline loops are filled
/// so the cut leaves no hole there. Linear in the mesh size apart from filling the outline.
pub fn cut(mesh: &Mesh, plane: &Plane, cap: bool) -> CutPieces {
    let count = mesh.vertex_count();
    // A vertex within a few f32 steps of the plane is on it. Deciding that once per vertex gives
    // every triangle around it the same answer, which is what keeps the pieces watertight.
    let largest = mesh
        .positions
        .iter()
        .fold(1.0f64, |m, v| m.max((*v as f64).abs()));
    let eps = 4.0 * f32::EPSILON as f64 * largest;
    let mut dist = Vec::with_capacity(count);
    let mut side = Vec::with_capacity(count);
    for v in 0..count as u32 {
        let s = dot(plane.normal, mesh.point(v)) - plane.offset;
        dist.push(s);
        side.push(if s > eps {
            ABOVE
        } else if s < -eps {
            BELOW
        } else {
            ON
        });
    }

    let mut crossings = Crossings {
        mesh,
        dist: &dist,
        positions: mesh.positions.clone(),
        ids: IdMap::default(),
    };
    let mut below: Vec<u32> = Vec::new();
    let mut above: Vec<u32> = Vec::new();
    let mut put = |s: i8, t: [u32; 3]| {
        if s == BELOW {
            below.extend_from_slice(&t);
        } else {
            above.extend_from_slice(&t);
        }
    };
    for t in mesh.indices.as_chunks::<3>().0 {
        let c = t.map(|i| side[i as usize]);
        match (c.contains(&BELOW), c.contains(&ABOVE)) {
            (true, false) => put(BELOW, *t),
            (false, true) => put(ABOVE, *t),
            (false, false) => {
                // Lies in the plane: a face looking along the normal is the top of the solid
                // below (and the bottom of nothing above), and the other way round.
                let [a, b, c] = t.map(|i| mesh.point(i));
                let facing = dot(cross(sub(b, a), sub(c, a)), plane.normal);
                put(if facing < 0.0 { ABOVE } else { BELOW }, *t);
            }
            (true, true) => {
                // Read the corners from k on, keeping the winding.
                let from = |k: usize| [t[k], t[(k + 1) % 3], t[(k + 2) % 3]];
                if let Some(k) = (0..3).find(|&k| c[k] == ON) {
                    // One corner on the plane; the edge opposite it crosses.
                    let [o, p, m] = from(k);
                    let q = crossings.at(p, m);
                    put(side[p as usize], [o, p, q]);
                    put(side[m as usize], [o, q, m]);
                } else {
                    // One corner alone on its side: a triangle there, a quad on the other.
                    let lone = if c[0] == c[1] {
                        2
                    } else if c[0] == c[2] {
                        1
                    } else {
                        0
                    };
                    let [l, x, y] = from(lone);
                    let q1 = crossings.at(l, x);
                    let q2 = crossings.at(l, y);
                    put(side[l as usize], [l, q1, q2]);
                    put(side[x as usize], [q1, x, y]);
                    put(side[x as usize], [q1, y, q2]);
                }
            }
        }
    }
    let mut positions = crossings.positions;

    let mut open_outline = false;
    if cap {
        let on = |v: u32| v as usize >= count || side[v as usize] == ON;
        let (touching, [below_edges, above_edges]) = cut_edges(&below, &above, on);
        let basis = basis(plane.normal);
        // A few f32 steps at the part's scale: an outline point this close to the straight line
        // between its neighbours is only rounding off that line (see `simplify`).
        let tol = 2.0 * f32::EPSILON as f64 * largest;
        let below_cap = fill(&below_edges, &touching, &positions, basis, tol);
        positions.extend(below_cap.points.iter().flatten());
        // A closed manifold has the same cut edges on both sides, so both can share one fill.
        let own_above_cap = if same_edges(&below_edges, &above_edges) {
            None
        } else {
            let cap = fill(&above_edges, &touching, &positions, basis, tol);
            positions.extend(cap.points.iter().flatten());
            Some(cap)
        };
        let above_cap = own_above_cap.as_ref().unwrap_or(&below_cap);
        below.extend(close(&below_cap, &below_edges, false));
        above.extend(close(above_cap, &above_edges, true));
        open_outline = below_cap.open || above_cap.open;
    }

    let piece = |indices: Vec<u32>| (!indices.is_empty()).then(|| compact(&positions, indices));
    CutPieces {
        below: piece(below),
        above: piece(above),
        open_outline,
    }
}

/// Intersection vertices, one per crossing edge, appended after the mesh's own vertices.
struct Crossings<'m> {
    mesh: &'m Mesh,
    dist: &'m [f64],
    positions: Vec<f32>,
    ids: IdMap<Edge, u32>,
}

impl Crossings<'_> {
    /// The vertex where edge a-b (corners strictly on opposite sides) meets the plane. Computed
    /// from the lower index so both triangles of the edge get the same rounded point.
    fn at(&mut self, a: u32, b: u32) -> u32 {
        let (lo, hi) = (a.min(b), a.max(b));
        let next = (self.positions.len() / 3) as u32;
        let id = *self.ids.entry((lo, hi)).or_insert(next);
        if id == next {
            let (p, q) = (self.mesh.point(lo), self.mesh.point(hi));
            let (s, t) = (self.dist[lo as usize], self.dist[hi as usize]);
            let f = s / (s - t);
            for k in 0..3 {
                self.positions.push((p[k] + (q[k] - p[k]) * f) as f32);
            }
        }
        id
    }
}

/// How the triangles of each side use one edge whose ends are both on the plane.
#[derive(Clone, Copy, Default)]
struct Uses {
    below: u32,
    above: u32,
    /// The direction of the last use on each side: true when it runs from the lower index.
    below_forward: bool,
    above_forward: bool,
}

/// Counts every edge of the pieces that lies in the plane (both ends on it) per side. Returns
/// where the part touches the plane, the in-plane edges that are not a cut edge of both sides
/// (which no inner cap edge may repeat), and the cut edges of each side, directed as in that
/// side's triangle: edges in the plane used by exactly one triangle of the side and by the other
/// side too. A rim of an open part that only happens to lie in the plane is not a cut edge, so
/// capping never closes a hole the part already had.
fn cut_edges(
    below: &[u32],
    above: &[u32],
    on: impl Fn(u32) -> bool,
) -> (IdSet<Edge>, [Vec<Edge>; 2]) {
    let mut uses: IdMap<Edge, Uses> = IdMap::default();
    for (triangles, is_below) in [(below, true), (above, false)] {
        for t in triangles.as_chunks::<3>().0 {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if a == b || !on(a) || !on(b) {
                    continue;
                }
                let e = uses.entry((a.min(b), a.max(b))).or_default();
                if is_below {
                    e.below += 1;
                    e.below_forward = a < b;
                } else {
                    e.above += 1;
                    e.above_forward = a < b;
                }
            }
        }
    }
    let mut touching = IdSet::default();
    let mut out = [Vec::new(), Vec::new()];
    for (&(lo, hi), e) in &uses {
        let directed = |forward: bool| if forward { (lo, hi) } else { (hi, lo) };
        if e.below == 1 && e.above >= 1 {
            out[0].push(directed(e.below_forward));
        }
        if e.above == 1 && e.below >= 1 {
            out[1].push(directed(e.above_forward));
        }
        if e.below != 1 || e.above != 1 {
            touching.insert((lo, hi));
        }
    }
    // HashMap order is random; sort so the same input always gives the same caps.
    out[0].sort_unstable();
    out[1].sort_unstable();
    (touching, out)
}

fn same_edges(a: &[Edge], b: &[Edge]) -> bool {
    let key = |&(x, y): &Edge| (x.min(y), x.max(y));
    a.len() == b.len() && {
        let mut a: Vec<_> = a.iter().map(key).collect();
        let mut b: Vec<_> = b.iter().map(key).collect();
        a.sort_unstable();
        b.sort_unstable();
        a == b
    }
}

/// An orthonormal basis (u, w) of the plane with u × w = normal, so counter-clockwise in (u, w)
/// faces along the normal.
fn basis(n: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let a = if n[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let u = cross(a, n);
    let length = dot(u, u).sqrt();
    let u = [u[0] / length, u[1] / length, u[2] / length];
    (u, cross(n, u))
}

// ---------------------------------------------------------------------------------------------
// Capping

/// The fill of one side's cut outline, before it is turned to close that side.
#[derive(Default)]
struct Cap {
    /// Counter-clockwise in the plane basis (facing +n).
    triangles: Vec<[u32; 3]>,
    /// The polygon (an outer loop with its holes) each triangle belongs to.
    polygon: Vec<u32>,
    /// Some cut edge has no cap triangle (an open chain, or a loop that could not be filled), or
    /// some cap edge is not shared by exactly two triangles.
    open: bool,
    /// Vertices the cap adds, numbered on from the positions `fill` was given.
    points: Vec<[f32; 3]>,
}

/// Fills the closed loops of one side's cut edges. `touching` holds the part's own edges in the
/// plane, which no inner cap edge may repeat; `positions` are the vertices so far.
///
/// Each polygon (an outer loop with its holes, see `nest`) is filled on its own by `fill_polygon`,
/// which keeps a fill only when it closes that polygon. Every step of the fast fill stays close to
/// linear in the outline size: the loops are nested through a grid of their boxes (`Containers`),
/// points that only continue a straight run are left out of the triangulation (`simplify`) and
/// fanned back in afterwards (`mend_t_junctions`), and a polygon with many holes has them linked
/// to its outer loop through a grid (`bridge_holes`). Earcut itself is left with simple polygons;
/// on those it ran near-linear in every case measured (a 1M-triangle cylinder cut lengthwise, a
/// 40,401-pillar lattice and a plate with 22,500 holes each cap in well under a second in a
/// release build). Only a polygon the fast fill does not close pays for the plain one.
fn fill(
    edges: &[Edge],
    touching: &IdSet<Edge>,
    positions: &[f32],
    (u, w): ([f64; 3], [f64; 3]),
    tol: f64,
) -> Cap {
    if edges.is_empty() {
        return Cap::default();
    }
    let flat = |v: u32| {
        let i = v as usize * 3;
        let p = [
            positions[i] as f64,
            positions[i + 1] as f64,
            positions[i + 2] as f64,
        ];
        [dot(p, u), dot(p, w)]
    };
    let (loops, mut open) = loops(edges, &flat);
    let rings: Vec<Ring> = loops.into_iter().map(|l| Ring::new(l, &flat)).collect();
    // A loop with no area to fill keeps its edges uncapped.
    open |= rings.iter().any(Ring::degenerate);
    let mut cap = Cap::default();
    for (n, (outer, holes)) in nest(&rings).into_iter().enumerate() {
        let members: Vec<usize> = once(outer).chain(holes).collect();
        let tol = polygon_tol(&rings[outer], positions, tol);
        let (part, closed) = fill_polygon(
            &rings,
            &members,
            tol,
            &flat,
            touching,
            positions,
            &mut cap.points,
        );
        open |= !closed;
        cap.polygon.resize(cap.polygon.len() + part.len(), n as u32);
        cap.triangles.extend(part);
    }
    cap.open = open;
    cap
}

/// The tolerance `simplify` may drop a polygon's points at: `tol` (a few f32 steps at the part's
/// scale), never more than 1e-5 of the polygon's size, and none at all when that is below the
/// rounding of the polygon's own coordinates (half an f32 step at its largest one). A small
/// outline far from the origin is all rounding, and keeps every point; above that, the mending
/// tolerance (twice this) covers a whole f32 step, so the points rounding scattered off a line are
/// found again. (At a millionth, a 20 mm outline 1 m from the origin kept every point: a cut along
/// a 1M-triangle cylinder there took 90 s and earcut still left gaps.)
fn polygon_tol(outer: &Ring, positions: &[f32], tol: f64) -> f64 {
    let extent = (outer.max[0] - outer.min[0]).max(outer.max[1] - outer.min[1]);
    let tol = tol.min(1e-5 * extent);
    let reach = outer
        .ids
        .iter()
        .flat_map(|&v| &positions[v as usize * 3..v as usize * 3 + 3])
        .fold(0.0f64, |m, &x| m.max((x as f64).abs()));
    if tol >= 0.5 * f32::EPSILON as f64 * reach {
        tol
    } else {
        0.0
    }
}

/// Fills one polygon, `members` being its outer loop and then its holes. Returns the cap
/// triangles (counter-clockwise in the plane basis) and whether they close the polygon: each edge
/// of its loops used by one cap triangle, every other cap edge by two that run it opposite ways.
/// New vertices (see `avoid_part_edges`) go to `points`.
///
/// The fast fill straightens runs and bridges holes on its own, and on outlines of points in
/// lines (a grid of holes, a finely divided face, all f32-rounded off their lines) that can leave
/// a T-junction it mends wrongly. So a fast fill that does not close the polygon is done again
/// the plain way, as earcut alone would: every point, no straightening or bridging, and only
/// T-junctions that a chain of outline edges explains mended. Whichever fill closes is kept, the
/// fast one when both do, so the fast fill only ever saves time; it is never less closed.
fn fill_polygon(
    rings: &[Ring],
    members: &[usize],
    tol: f64,
    flat: &dyn Fn(u32) -> [f64; 2],
    touching: &IdSet<Edge>,
    positions: &[f32],
    points: &mut Vec<[f32; 3]>,
) -> (Vec<[u32; 3]>, bool) {
    let (outer, holes) = (members[0], &members[1..]);
    let mut cap = triangulate(rings, outer, holes, tol, true);
    let mut uses = edge_uses(rings, members, &cap);
    drop_flat(&mut cap, &mut uses, rings, members, flat);
    // Twice the tolerance `simplify` drops points at, so rounding never loses one.
    mend_t_junctions(&mut cap, &mut uses, rings, members, flat, Some(2.0 * tol));
    let mut flawed = flaws(&cap, &uses);
    if flawed > 0 {
        let mut plain = triangulate(rings, outer, holes, 0.0, false);
        let mut plain_uses = edge_uses(rings, members, &plain);
        mend_t_junctions(&mut plain, &mut plain_uses, rings, members, flat, None);
        let plain_flawed = flaws(&plain, &plain_uses);
        if plain_flawed <= flawed {
            (cap, uses, flawed) = (plain, plain_uses, plain_flawed);
        }
    }
    if !touching.is_empty() {
        let moved = avoid_part_edges(&mut cap, &mut uses, touching, flat, positions, points);
        flawed += usize::from(!moved);
        if flawed == 0 {
            flawed = flaws(&cap, &uses);
        }
    }
    (cap, flawed == 0)
}

/// How far a polygon's fill is from closing it (0 when it does): the edges not used exactly twice
/// (see `edge_uses`) and, when there are none, the cap edges two cap triangles run the same way.
fn flaws(cap: &[[u32; 3]], uses: &IdMap<Edge, u32>) -> usize {
    let wrong = uses.values().filter(|&&n| n != 2).count();
    if wrong > 0 {
        return wrong;
    }
    let mut runs: IdSet<Edge> = IdSet::with_capacity_and_hasher(cap.len() * 3, Default::default());
    cap.iter()
        .flat_map(|t| (0..3).map(move |k| (t[k], t[(k + 1) % 3])))
        .filter(|&e| !runs.insert(e))
        .count()
}

/// The cap's triangles (as indices) turned to close the side whose cut edges are `edges`,
/// directed as in that side's triangles. A consistently wound piece needs each cap triangle to
/// run its outline edge against the side triangle's, so each polygon goes the way most of its
/// outline edges ask: +n under the cut and -n over it for an outward-facing part, the other way for
/// an inside-out one. Only when the outline gives no answer does the side decide.
fn close(cap: &Cap, edges: &[Edge], above: bool) -> Vec<u32> {
    let mut owner: IdMap<Edge, u32> =
        IdMap::with_capacity_and_hasher(cap.triangles.len() * 3, Default::default());
    for (t, &p) in cap.triangles.iter().zip(&cap.polygon) {
        for k in 0..3 {
            owner.insert((t[k], t[(k + 1) % 3]), p);
        }
    }
    let polygons = cap.polygon.iter().max().map_or(0, |&p| p as usize + 1);
    // (against, along) votes per polygon.
    let mut votes = vec![(0u32, 0u32); polygons];
    for &(a, b) in edges {
        if let Some(&p) = owner.get(&(b, a)) {
            votes[p as usize].0 += 1;
        } else if let Some(&p) = owner.get(&(a, b)) {
            votes[p as usize].1 += 1;
        }
    }
    let turn: Vec<bool> = votes
        .iter()
        .map(|&(against, along)| {
            if against == along {
                above
            } else {
                along > against
            }
        })
        .collect();
    cap.triangles
        .iter()
        .zip(&cap.polygon)
        .flat_map(|(t, &p)| {
            if turn[p as usize] {
                [t[0], t[2], t[1]]
            } else {
                *t
            }
        })
        .collect()
}

/// One closed loop of cut edges.
struct Loop {
    /// Vertex ids in walk order.
    ids: Vec<u32>,
    /// Whether that side's triangles run every edge of the loop the way the walk goes (true), every
    /// edge the other way (false), or some each way (None: a part wound inconsistently there).
    along: Option<bool>,
    /// Other cut edges meet at one of its vertices: loops touching there, or a loose chain.
    touches: bool,
}

/// Splits the cut edges into closed loops; returns them and whether any edge was left over on an
/// open chain (an outline that ends somewhere, as on an open or damaged part).
///
/// Chains that end are peeled off first, so a loose chain touching a loop cannot break the loop.
/// The walk then prefers the edge that continues in the same direction, and a vertex reached a
/// second time closes the loop back to it, so outlines that touch themselves come out as
/// separate simple loops.
///
/// Where several such edges leave a vertex, the walk takes the one that turns furthest towards
/// the cap (the side's net outline area says which side of its edges that is), so each loop
/// goes round one piece of the cap. A hole that touches its outline at two or more points then
/// comes out as the pieces between them rather than as a hole earcut cannot fill.
fn loops(edges: &[Edge], flat: &dyn Fn(u32) -> [f64; 2]) -> (Vec<Loop>, bool) {
    const NONE: u32 = u32::MAX;
    // Local vertex numbers and the edges at each vertex.
    let mut local: IdMap<u32, u32> =
        IdMap::with_capacity_and_hasher(edges.len(), Default::default());
    let mut ids = Vec::with_capacity(edges.len());
    let mut ends = Vec::with_capacity(edges.len());
    for &(a, b) in edges {
        let mut id = |v: u32| {
            *local.entry(v).or_insert_with(|| {
                ids.push(v);
                ids.len() as u32 - 1
            })
        };
        ends.push([id(a), id(b)]);
    }
    let count = ids.len();
    let mut start = vec![0usize; count + 1];
    for e in &ends {
        start[e[0] as usize + 1] += 1;
        start[e[1] as usize + 1] += 1;
    }
    for v in 0..count {
        start[v + 1] += start[v];
    }
    let mut fill = start[..count].to_vec();
    let mut incident = vec![0u32; start[count]];
    for (n, e) in ends.iter().enumerate() {
        for &v in e {
            incident[fill[v as usize]] = n as u32;
            fill[v as usize] += 1;
        }
    }
    let at = |v: u32| &incident[start[v as usize]..start[v as usize + 1]];
    let mut degree: Vec<u32> = (0..count)
        .map(|v| (start[v + 1] - start[v]) as u32)
        .collect();
    let mut used = vec![false; ends.len()];
    let mut open = false;

    let take = |e: u32, used: &mut [bool], degree: &mut [u32]| {
        used[e as usize] = true;
        for v in ends[e as usize] {
            degree[v as usize] -= 1;
        }
    };
    let mut peel: Vec<u32> = (0..count as u32)
        .filter(|&v| degree[v as usize] == 1)
        .collect();
    while let Some(v) = peel.pop() {
        if degree[v as usize] != 1 {
            continue;
        }
        let Some(&e) = at(v).iter().find(|&&e| !used[e as usize]) else {
            continue;
        };
        take(e, &mut used, &mut degree);
        open = true;
        let [a, b] = ends[e as usize];
        let other = if a == v { b } else { a };
        if degree[other as usize] == 1 {
            peel.push(other);
        }
    }

    let mut out = Vec::new();
    let mut position = vec![NONE; count];
    let mut cap_on_right: Option<bool> = None;
    let mut path: Vec<u32> = Vec::new();
    // Whether the edge into each path vertex runs the way its triangle does (none into the first).
    let mut forward: Vec<bool> = Vec::new();
    for first in 0..ends.len() {
        if used[first] {
            continue;
        }
        let mut current = ends[first][0];
        path.clear();
        forward.clear();
        path.push(current);
        forward.push(true);
        position[current as usize] = 0;
        loop {
            let free = at(current).iter().copied().filter(|&e| !used[e as usize]);
            let mut leaving = free.clone().filter(|&e| ends[e as usize][0] == current);
            let first = leaving.next();
            let choice = if first.is_some() && leaving.next().is_some() && path.len() > 1 {
                // The cap lies right of every edge when the outline's net area is negative.
                let right = *cap_on_right.get_or_insert_with(|| {
                    let area: f64 = edges
                        .iter()
                        .map(|&(a, b)| {
                            let (p, q) = (flat(a), flat(b));
                            p[0] * q[1] - q[0] * p[1]
                        })
                        .sum();
                    area < 0.0
                });
                let here = flat(ids[current as usize]);
                let from = flat(ids[path[path.len() - 2] as usize]);
                let back = [from[0] - here[0], from[1] - here[1]];
                // Counter-clockwise from the way back, in [0, 2π); straight back comes last.
                let turn = |e: &u32| {
                    let to = flat(ids[ends[*e as usize][1] as usize]);
                    let d = [to[0] - here[0], to[1] - here[1]];
                    let mut a =
                        (back[0] * d[1] - back[1] * d[0]).atan2(back[0] * d[0] + back[1] * d[1]);
                    if a < 0.0 {
                        a += std::f64::consts::TAU;
                    }
                    if right && a == 0.0 {
                        a = std::f64::consts::TAU;
                    }
                    a
                };
                let leaving = free.clone().filter(|&e| ends[e as usize][0] == current);
                if right {
                    leaving.min_by(|a, b| turn(a).total_cmp(&turn(b)))
                } else {
                    leaving.max_by(|a, b| turn(a).total_cmp(&turn(b)))
                }
            } else {
                first
            };
            let Some(e) = choice.or_else(|| free.clone().next()) else {
                break;
            };
            take(e, &mut used, &mut degree);
            let [a, b] = ends[e as usize];
            let ahead = a == current;
            let next = if ahead { b } else { a };
            let seen = position[next as usize];
            if seen == NONE {
                position[next as usize] = path.len() as u32;
                path.push(next);
                forward.push(ahead);
            } else {
                let s = seen as usize;
                let runs = || forward[s + 1..].iter().copied().chain(once(ahead));
                let along = if runs().all(|f| f) {
                    Some(true)
                } else if runs().all(|f| !f) {
                    Some(false)
                } else {
                    None
                };
                let walked = &path[s..];
                out.push(Loop {
                    ids: walked.iter().map(|&v| ids[v as usize]).collect(),
                    along,
                    touches: walked
                        .iter()
                        .any(|&v| start[v as usize + 1] - start[v as usize] > 2),
                });
                for &v in &path[s + 1..] {
                    position[v as usize] = NONE;
                }
                path.truncate(s + 1);
                forward.truncate(s + 1);
            }
            current = next;
        }
        if path.len() > 1 {
            open = true;
        }
        for &v in &path {
            position[v as usize] = NONE;
        }
    }
    (out, open)
}

/// One closed outline loop in the plane basis.
struct Ring {
    ids: Vec<u32>,
    points: Vec<[f64; 2]>,
    /// Twice the signed area, in loop order.
    area: f64,
    min: [f64; 2],
    max: [f64; 2],
    /// As in `Loop`.
    along: Option<bool>,
    touches: bool,
}

impl Ring {
    fn new(l: Loop, flat: &dyn Fn(u32) -> [f64; 2]) -> Ring {
        let points: Vec<[f64; 2]> = l.ids.iter().map(|&v| flat(v)).collect();
        let mut area = 0.0;
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        for (i, p) in points.iter().enumerate() {
            let q = points[(i + 1) % points.len()];
            area += p[0] * q[1] - q[0] * p[1];
            for k in 0..2 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        Ring {
            ids: l.ids,
            points,
            area,
            min,
            max,
            along: l.along,
            touches: l.touches,
        }
    }

    /// Twice the signed area as that side's triangles run the loop, when they all run it one way.
    fn facing(&self) -> Option<f64> {
        self.along
            .map(|along| if along { self.area } else { -self.area })
    }

    /// Undirected edge keys, as `edge_uses` counts them.
    fn edges(&self) -> impl Iterator<Item = Edge> + '_ {
        (0..self.ids.len()).map(|i| key(self.ids[i], self.ids[(i + 1) % self.ids.len()]))
    }

    /// A loop with no area to fill: too few points, a line at f64 precision, or coordinates that
    /// are not numbers.
    fn degenerate(&self) -> bool {
        let extent = (self.max[0] - self.min[0]).max(self.max[1] - self.min[1]);
        // Written so that an area that is not a number has no area either.
        let has_area = self.area.abs() > f64::EPSILON * extent * extent;
        self.ids.len() < 3 || !has_area
    }

    /// Even-odd test. `rows`, when given, lists the edges a ray along +u at each w can cross.
    fn contains(&self, p: [f64; 2], rows: Option<&Rows>) -> bool {
        if p[0] < self.min[0] || p[0] > self.max[0] || p[1] < self.min[1] || p[1] > self.max[1] {
            return false;
        }
        let n = self.points.len();
        let crosses = |i: usize| {
            let (a, b) = (self.points[i], self.points[(i + 1) % n]);
            (a[1] > p[1]) != (b[1] > p[1])
                && p[0] < a[0] + (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1])
        };
        let crossings = match rows {
            Some(rows) => rows
                .at(p[1])
                .iter()
                .filter(|&&i| crosses(i as usize))
                .count(),
            None => (0..n).filter(|&i| crosses(i)).count(),
        };
        crossings % 2 == 1
    }

    /// The point `nest` tests against the other loops: the midpoint of the first edge rather than a
    /// vertex, so loops that touch at a vertex still nest correctly.
    fn probe(&self) -> [f64; 2] {
        let (a, b) = (self.points[0], self.points[1 % self.points.len()]);
        [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
    }
}

/// A ring's edges listed under rows of w, so an even-odd test only looks at the edges its ray can
/// cross rather than all of them.
struct Rows {
    lo: f64,
    scale: f64,
    count: usize,
    start: Vec<usize>,
    edges: Vec<u32>,
}

impl Rows {
    fn new(ring: &Ring) -> Rows {
        let n = ring.points.len();
        let lo = ring.min[1];
        let height = ring.max[1] - lo;
        let row = |w: f64, scale: f64, count: usize| (((w - lo) * scale) as usize).min(count - 1);
        let span = |i: usize, scale: f64, count: usize| {
            let (a, b) = (ring.points[i][1], ring.points[(i + 1) % n][1]);
            let first = row(a.min(b), scale, count);
            (first, row(a.max(b), scale, count).max(first))
        };
        // About four edges a row, fewer rows if long edges would be listed under too many.
        let mut count = (n / 4).max(1);
        while count > 1 {
            let scale = count as f64 / height;
            let listed: usize = (0..n)
                .map(|i| {
                    let (first, last) = span(i, scale, count);
                    last - first + 1
                })
                .sum();
            if listed <= 8 * n {
                break;
            }
            count /= 2;
        }
        let scale = count as f64 / height;
        let mut start = vec![0usize; count + 1];
        for i in 0..n {
            let (first, last) = span(i, scale, count);
            for r in first..=last {
                start[r + 1] += 1;
            }
        }
        for r in 0..count {
            start[r + 1] += start[r];
        }
        let mut fill = start.clone();
        let mut edges = vec![0u32; start[count]];
        for i in 0..n {
            let (first, last) = span(i, scale, count);
            for r in first..=last {
                edges[fill[r]] = i as u32;
                fill[r] += 1;
            }
        }
        Rows {
            lo,
            scale,
            count,
            start,
            edges,
        }
    }

    fn at(&self, w: f64) -> &[u32] {
        let r = self.row(w);
        &self.edges[self.start[r]..self.start[r + 1]]
    }

    /// The edges listed in the rows from w = lo to w = hi (an edge may be listed more than once).
    fn between(&self, lo: f64, hi: f64) -> &[u32] {
        let (first, last) = (self.row(lo), self.row(hi));
        &self.edges[self.start[first]..self.start[last.max(first) + 1]]
    }

    fn row(&self, w: f64) -> usize {
        (((w - self.lo) * self.scale) as usize).min(self.count - 1)
    }
}

/// Groups the loops into polygons, an outer loop with the holes directly inside it, as indices
/// into `rings`; degenerate loops take no part.
///
/// Which loops are holes follows from the way that side's triangles run them: on a consistently
/// wound shell the outer loops run one way and the loops around its cavities the other. The way
/// outer loops run is read off the loops that lie inside no other (they can only be outer
/// boundaries), so a part wound inside out caps as well. Each hole goes to the smallest outer
/// loop that holds all of it (`encloses`): a cavity that crosses another shell's outline belongs to
/// its own shell's loop around both. Loops of different shells that overlap or lie inside each
/// other but run the same way are separate polygons, and their caps overlap as the shells do. A
/// loop whose edges run both ways (a part wound inconsistently) falls back to nesting depth:
/// inside an even number of other loops it is an outer one; so does every loop when no loop that
/// lies inside no other runs one way. A hole no outer loop holds is filled on its own, and `close`
/// turns it the way its edges ask.
///
/// Not handled: a part whose shells disagree in winding where it matters, as an inside-out shell
/// with a cavity beside a larger outward one. The minority shell's cavity then caps as if solid.
///
/// Memory stays linear in the number of loops: per loop only how many loops it lies inside and the
/// smallest of them per way they run are kept. Time is not: L loops all inside each other (nested
/// tubes merged into one part) cost L²/2 point tests, about a second for 10,000 loops.
fn nest(rings: &[Ring]) -> Vec<(usize, Vec<usize>)> {
    const NONE: u32 = u32::MAX;
    let live: Vec<usize> = (0..rings.len())
        .filter(|&r| !rings[r].degenerate())
        .collect();
    let mut rows: Vec<Option<Rows>> = (0..rings.len()).map(|_| None).collect();
    let index = Containers::new(rings, &live);
    // Per ring: how many loops its probe lies inside, and the smallest of those that run each
    // way (by `way`).
    let mut depth = vec![0u32; rings.len()];
    let mut smallest = vec![[NONE; 3]; rings.len()];
    let size = |r: usize| rings[r].area.abs();
    for &r in &live {
        index.around(rings, r, &mut rows, &mut |o| {
            depth[r] += 1;
            let best = &mut smallest[r][way(&rings[o])];
            if *best == NONE || size(o) < size(*best as usize) {
                *best = o as u32;
            }
        });
    }
    let vote: f64 = live
        .iter()
        .filter(|&&r| depth[r] == 0)
        .filter_map(|&r| rings[r].facing())
        .sum();
    let outer = |r: usize| match rings[r].facing() {
        Some(facing) if vote != 0.0 => (facing > 0.0) == (vote > 0.0),
        _ => depth[r].is_multiple_of(2),
    };
    let mut polygons: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut slot = vec![usize::MAX; rings.len()];
    for &r in live.iter().filter(|&&r| outer(r)) {
        slot[r] = polygons.len();
        polygons.push((r, Vec::new()));
    }
    let holes: Vec<usize> = live.iter().copied().filter(|&r| !outer(r)).collect();
    if holes.is_empty() {
        return polygons;
    }
    let shared = shared_vertices(rings);
    let mut found = Vec::new();
    for r in holes {
        // With every loop wound one way, the smallest loop around r that runs the outer way is
        // the first choice, and usually holds all of r. Otherwise every outer loop around r is
        // tried, smallest first.
        let known = vote != 0.0 && smallest[r][2] == NONE;
        let first = smallest[r][if vote > 0.0 { 0 } else { 1 }];
        let parent = if known && first == NONE {
            None
        } else if known && encloses(rings, first as usize, r, &mut rows, &shared) {
            Some(first as usize)
        } else {
            found.clear();
            index.around(rings, r, &mut rows, &mut |o| {
                if outer(o) {
                    found.push(o);
                }
            });
            found.sort_by(|&a, &b| size(a).total_cmp(&size(b)));
            found
                .iter()
                .copied()
                .find(|&o| encloses(rings, o, r, &mut rows, &shared))
        };
        match parent {
            Some(p) => polygons[slot[p]].1.push(r),
            None => polygons.push((r, Vec::new())),
        }
    }
    polygons
}

/// Which way a loop runs, for `nest`: 0 with a positive `facing`, 1 otherwise, 2 for both ways.
fn way(ring: &Ring) -> usize {
    match ring.facing() {
        Some(f) if f > 0.0 => 0,
        Some(_) => 1,
        None => 2,
    }
}

/// The vertices of more than one loop: where loops touch.
fn shared_vertices(rings: &[Ring]) -> IdSet<u32> {
    let mut count: IdMap<u32, u8> = IdMap::default();
    for ring in rings.iter().filter(|r| r.touches) {
        for &v in &ring.ids {
            let n = count.entry(v).or_default();
            *n = n.saturating_add(1);
        }
    }
    count
        .into_iter()
        .filter(|&(_, n)| n > 1)
        .map(|(v, _)| v)
        .collect()
}

/// Whether ring o holds the whole of ring r: r's box lies in o's, and every point of r lies
/// inside o, apart from the ones where loops touch (`shared`), which lie on it. A point of r on
/// o's outline (within a few f32 steps) does not count as inside: two shells whose walls meet in
/// a line, as a cavity wall flush with another shell's side, leave a hole earcut cannot fill
/// there, and the hole belongs to a loop around both.
fn encloses(
    rings: &[Ring],
    o: usize,
    r: usize,
    rows: &mut [Option<Rows>],
    shared: &IdSet<u32>,
) -> bool {
    let (outer, hole) = (&rings[o], &rings[r]);
    let boxed = (0..2).all(|k| outer.min[k] <= hole.min[k] && hole.max[k] <= outer.max[k]);
    if !boxed {
        return false;
    }
    let reach = (0..2).fold(0.0f64, |m, k| {
        m.max(outer.min[k].abs()).max(outer.max[k].abs())
    });
    let near = 4.0 * f32::EPSILON as f64 * reach;
    hole.points.iter().zip(&hole.ids).all(|(&p, v)| {
        shared.contains(v) || (inside(rings, o, p, rows) && !beside(rings, o, p, near, rows))
    }) && !reaches_into(rings, o, r, near, rows, shared)
}

/// Whether a point of ring o lies strictly inside ring r (not on r's outline, not where loops
/// touch). Every point of r can lie inside o while o's outline still runs through r, as a U-shaped
/// shell whose slot opens into a cavity; o then does not hold r. Only o's points level with r are
/// looked at, through o's `Rows`.
fn reaches_into(
    rings: &[Ring],
    o: usize,
    r: usize,
    near: f64,
    rows: &mut [Option<Rows>],
    shared: &IdSet<u32>,
) -> bool {
    let (outer, hole) = (&rings[o], &rings[r]);
    let candidates: Vec<u32> = if outer.points.len() <= 32 {
        (0..outer.points.len() as u32).collect()
    } else {
        let index = rows[o].get_or_insert_with(|| Rows::new(outer));
        index.between(hole.min[1], hole.max[1]).to_vec()
    };
    candidates.into_iter().any(|i| {
        let (p, v) = (outer.points[i as usize], outer.ids[i as usize]);
        let boxed = (0..2).all(|k| hole.min[k] < p[k] && p[k] < hole.max[k]);
        boxed
            && !shared.contains(&v)
            && inside(rings, r, p, rows)
            && !beside(rings, r, p, near, rows)
    })
}

/// Whether p lies within `near` of an edge of ring o.
fn beside(rings: &[Ring], o: usize, p: [f64; 2], near: f64, rows: &mut [Option<Rows>]) -> bool {
    let ring = &rings[o];
    let n = ring.points.len();
    let close = |i: u32| {
        let (a, b) = (ring.points[i as usize], ring.points[(i as usize + 1) % n]);
        let d = [b[0] - a[0], b[1] - a[1]];
        let r = [p[0] - a[0], p[1] - a[1]];
        let length2 = d[0] * d[0] + d[1] * d[1];
        let t = ((r[0] * d[0] + r[1] * d[1]) / length2).clamp(0.0, 1.0);
        let off = [r[0] - d[0] * t, r[1] - d[1] * t];
        // A zero-length edge gives t = NaN: its distance is the one to a.
        let off = if t.is_nan() { r } else { off };
        off[0].hypot(off[1]) <= near
    };
    if n <= 32 {
        return (0..n as u32).any(close);
    }
    let index = rows[o].get_or_insert_with(|| Rows::new(ring));
    // An edge within near of p is listed in the row of p's w, or of a w within near of it
    // (nearly always the same row).
    let (low, here, high) = (index.at(p[1] - near), index.at(p[1]), index.at(p[1] + near));
    let other = |row: &[u32]| !std::ptr::eq(row, here) && row.iter().any(|&i| close(i));
    here.iter().any(|&i| close(i)) || other(low) || other(high)
}

/// Even-odd test of p against ring o, through its `Rows` (built the first time) when it has many
/// points.
fn inside(rings: &[Ring], o: usize, p: [f64; 2], rows: &mut [Option<Rows>]) -> bool {
    let ring = &rings[o];
    let index = (ring.points.len() > 32).then(|| &*rows[o].get_or_insert_with(|| Rows::new(ring)));
    ring.contains(p, index)
}

/// The live rings' boxes in a grid, for finding the rings around a probe point: only the few
/// listed in its cell are tested, and a ring with many points answers through its `Rows`, so
/// neither many small loops (a lattice of 40,000 pillars) nor a few huge ones cost a test of every
/// probe against every edge.
struct Containers {
    grid: Grid,
    start: Vec<usize>,
    listed: Vec<u32>,
}

impl Containers {
    fn new(rings: &[Ring], live: &[usize]) -> Containers {
        let mut grid = Grid::new(live.iter().flat_map(|&r| [rings[r].min, rings[r].max]), 0.0);
        let cells =
            |grid: &Grid, r: usize| (grid.cell_of(rings[r].min), grid.cell_of(rings[r].max));
        let area = |(a, b): ([usize; 2], [usize; 2])| {
            (b[0].saturating_sub(a[0]) + 1) * (b[1].saturating_sub(a[1]) + 1)
        };
        // Big boxes are listed in many cells; coarser cells until that stays a few per ring.
        while grid.len() > 1 {
            let listed: usize = live.iter().map(|&r| area(cells(&grid, r))).sum();
            if listed <= 16 * live.len() + grid.len() {
                break;
            }
            grid = grid.coarser();
        }
        let mut start = vec![0usize; grid.len() + 1];
        let each_cell = |grid: &Grid, r: usize, f: &mut dyn FnMut(usize)| {
            let (a, b) = cells(grid, r);
            for y in a[1]..=b[1].max(a[1]) {
                for x in a[0]..=b[0].max(a[0]) {
                    f(grid.index([x, y]));
                }
            }
        };
        for &r in live {
            each_cell(&grid, r, &mut |c| start[c + 1] += 1);
        }
        for c in 0..grid.len() {
            start[c + 1] += start[c];
        }
        let mut fill = start.clone();
        let mut listed = vec![0u32; start[grid.len()]];
        for &r in live {
            each_cell(&grid, r, &mut |c| {
                listed[fill[c]] = r as u32;
                fill[c] += 1;
            });
        }
        Containers {
            grid,
            start,
            listed,
        }
    }

    /// Calls `each` with every live ring other than r that r's probe point lies inside.
    fn around(
        &self,
        rings: &[Ring],
        r: usize,
        rows: &mut [Option<Rows>],
        each: &mut dyn FnMut(usize),
    ) {
        let p = rings[r].probe();
        let c = self.grid.index(self.grid.cell_of(p));
        for &o in &self.listed[self.start[c]..self.start[c + 1]] {
            let o = o as usize;
            if o != r && inside(rings, o, p, rows) {
                each(o);
            }
        }
    }
}

/// Holes beyond this many are linked to their outer loop by `bridge_holes` rather than by earcut.
const MANY_HOLES: usize = 16;

/// Triangulates one polygon (an outer loop and its holes) with the loops' vertex ids,
/// counter-clockwise in the plane basis. Empty when earcut fails, which `fill_polygon` reports.
///
/// The `fast` way leaves out points that only continue a straight run (`simplify`), which
/// `mend_t_junctions` puts back, splices holes that touch the outer loop into it, and links many
/// holes through a grid (`bridge_holes`). A loop that touches another one (a hole meeting its
/// outline at a vertex, faces shared between shells) keeps all its points even so: straightening
/// the runs into the touching point leaves earcut a configuration it gets wrong. Otherwise earcut
/// gets every point of every loop, and links the holes itself.
fn triangulate(
    rings: &[Ring],
    outer: usize,
    holes: &[usize],
    tol: f64,
    fast: bool,
) -> Vec<[u32; 3]> {
    let mut coords: Vec<f64> = Vec::new();
    let mut ids: Vec<u32> = Vec::new();
    let mut starts = Vec::new();
    let (spliced, holes) = if fast {
        splice_touching_holes(rings, outer, holes)
    } else {
        (None, holes.to_vec())
    };
    let bridging = fast && holes.len() > MANY_HOLES;
    for (n, &r) in once(&outer).chain(&holes).enumerate() {
        if n > 0 {
            starts.push(ids.len());
        }
        if let (0, Some(spliced)) = (n, &spliced) {
            for &(r, i) in spliced {
                coords.extend(rings[r].points[i]);
                ids.push(rings[r].ids[i]);
            }
            continue;
        }
        let ring = &rings[r];
        let mut keep = if ring.touches || !fast {
            Vec::new()
        } else {
            simplify(&ring.points, tol)
        };
        if keep.len() < 3 {
            // Touching, or straight within tol all round: give earcut every point.
            keep = (0..ring.points.len()).collect();
        }
        for i in keep {
            coords.extend(ring.points[i]);
            ids.push(ring.ids[i]);
        }
    }
    let straighten = once(&outer).chain(&holes).all(|&r| !rings[r].touches);
    let linked = if bridging {
        bridge_holes(&coords, &starts).and_then(|pieces| {
            let mut triangles: Vec<usize> = Vec::new();
            for piece in pieces {
                // The links run in straight chains from hole to hole; those points go too.
                let points: Vec<[f64; 2]> = piece
                    .iter()
                    .map(|&i| [coords[2 * i], coords[2 * i + 1]])
                    .collect();
                let mut keep = if straighten {
                    simplify(&points, tol)
                } else {
                    Vec::new()
                };
                if keep.len() < 3 {
                    keep = (0..piece.len()).collect();
                }
                let along: Vec<f64> = keep.iter().flat_map(|&k| points[k]).collect();
                let cut = earcutr::earcut(&along, &[], 2).ok()?;
                triangles.extend(cut.into_iter().map(|k| piece[keep[k]]));
            }
            covers(&coords, &starts, &triangles).then_some(triangles)
        })
    } else {
        None
    };
    let Some(triangles) = linked.or_else(|| earcutr::earcut(&coords, &starts, 2).ok()) else {
        return Vec::new();
    };
    let at = |i: usize| [coords[2 * i], coords[2 * i + 1]];
    let mut area = 0.0;
    let mut part: Vec<[u32; 3]> = Vec::with_capacity(triangles.len() / 3);
    for t in triangles.as_chunks::<3>().0 {
        area += area2(t.map(at));
        part.push(t.map(|i| ids[i]));
    }
    // Earcut keeps its rings' winding in every triangle, so one sign check orients them all.
    if area < 0.0 {
        for t in &mut part {
            t.swap(1, 2);
        }
    }
    part
}

/// Earcut fails on a hole that touches its outline at a vertex. Spliced into the outer loop at that
/// vertex, the hole is part of the outline running out and back through it, as earcut's own links
/// are. Returns the outer loop as (ring, point index) pairs with every such hole spliced in (None
/// when no hole touches it), and the holes left over.
fn splice_touching_holes(
    rings: &[Ring],
    outer: usize,
    holes: &[usize],
) -> (Option<Vec<(usize, usize)>>, Vec<usize>) {
    let o = &rings[outer];
    if !o.touches || holes.is_empty() {
        return (None, holes.to_vec());
    }
    let at: IdMap<u32, usize> = o.ids.iter().enumerate().map(|(i, &v)| (v, i)).collect();
    // Per outer point, the holes that touch there: (hole, its point at that vertex).
    let mut touching: IdMap<usize, Vec<(usize, usize)>> = IdMap::default();
    let mut rest = Vec::new();
    for &h in holes {
        let ring = &rings[h];
        let shared = ring
            .touches
            .then(|| ring.ids.iter().position(|v| at.contains_key(v)))
            .flatten();
        match shared {
            Some(k) => touching.entry(at[&ring.ids[k]]).or_default().push((h, k)),
            None => rest.push(h),
        }
    }
    if touching.is_empty() {
        return (None, rest);
    }
    let mut out = Vec::new();
    for i in 0..o.ids.len() {
        out.push((outer, i));
        for &(h, k) in touching.get(&i).into_iter().flatten() {
            // Round the hole the other way from the outline, and back to the shared vertex.
            let n = rings[h].ids.len();
            let reverse = (rings[h].area > 0.0) == (o.area > 0.0);
            for s in 1..n {
                out.push((
                    h,
                    if reverse {
                        (k + n - s) % n
                    } else {
                        (k + s) % n
                    },
                ));
            }
            out.push((outer, i));
        }
    }
    (Some(out), rest)
}

/// The points of a closed ring that earcut needs, as indices in ring order. A point that only
/// continues a straight run, one whose removal moves the outline by no more than `tol`, is left
/// out: f32 rounding scatters the crossing points of a straight cut just off their line, and
/// earcut walks the whole of such a run for each triangle it cuts off near it (a 1M-triangle
/// cylinder cut lengthwise took 37 to 54 s).
///
/// Starts at a point that is surely a corner and stretches each segment as long as every point it
/// skips stays within `tol` of it. The directions the segment may take form a wedge that each
/// skipped point narrows, so the walk is linear, and the skipped points all lie strictly between
/// the segment's ends.
fn simplify(points: &[[f64; 2]], tol: f64) -> Vec<usize> {
    let n = points.len();
    let some_tol = tol > 0.0;
    if n <= 3 || !some_tol {
        return (0..n).collect();
    }
    // The lowest point in (u, w) order has both neighbours to one side of it: a corner.
    let start = (0..n)
        .min_by(|&a, &b| {
            let (p, q) = (points[a], points[b]);
            p[0].total_cmp(&q[0]).then(p[1].total_cmp(&q[1]))
        })
        .unwrap_or(0);
    let at = |k: usize| points[(start + k) % n];
    let cross2 = |a: [f64; 2], b: [f64; 2]| a[0] * b[1] - a[1] * b[0];
    let mut keep = vec![start];
    let mut anchor = 0;
    while anchor + 1 < n {
        let a = at(anchor);
        let from = |k: usize| [at(k)[0] - a[0], at(k)[1] - a[1]];
        let mut end = anchor + 1;
        // The wedge of directions from `a` that pass within tol of every point skipped so far,
        // and the furthest of those points.
        let mut wedge: Option<([f64; 2], [f64; 2])> = None;
        let mut reach = 0.0f64;
        while end < n {
            let e = from(end);
            let d = e[0].hypot(e[1]);
            if !(d > 0.0 && d.is_finite()) {
                break;
            }
            // Skipping `end` keeps it within tol of segments turned up to asin(s) from it; capped
            // at 30 degrees so a skipped point never sits beside the segment's start.
            let s = (tol / d).min(0.5);
            let c = (1.0 - s * s).sqrt();
            let unit = [e[0] / d, e[1] / d];
            let low = [unit[0] * c + unit[1] * s, unit[1] * c - unit[0] * s];
            let high = [unit[0] * c - unit[1] * s, unit[1] * c + unit[0] * s];
            let (lo, hi) = match wedge {
                None => (low, high),
                Some((lo, hi)) => (
                    if cross2(lo, low) > 0.0 { low } else { lo },
                    if cross2(high, hi) > 0.0 { high } else { hi },
                ),
            };
            wedge = Some((lo, hi));
            reach = reach.max(d);
            let q = from(end + 1);
            let fits = cross2(lo, q) >= 0.0
                && cross2(q, hi) >= 0.0
                && lo[0] * q[0] + lo[1] * q[1] > 0.0
                && q[0].hypot(q[1]) > reach;
            if !fits {
                break;
            }
            end += 1;
        }
        if end == n {
            // This segment runs back to the start.
            break;
        }
        keep.push((start + end) % n);
        anchor = end;
    }
    keep
}

/// Whether `triangles` (earcut indices into `coords`) tile the polygon `coords` / `starts` once:
/// their areas add up to the polygon's and none is turned over.
fn covers(coords: &[f64], starts: &[usize], triangles: &[usize]) -> bool {
    let n = coords.len() / 2;
    if n == 0 {
        return false;
    }
    // Relative to the first point, so a polygon far from the origin keeps its precision.
    let at = |i: usize| [coords[2 * i] - coords[0], coords[2 * i + 1] - coords[1]];
    let (mut want, mut scale) = (0.0, 0.0);
    let bounds = once(0)
        .chain(starts.iter().copied())
        .zip(starts.iter().copied().chain(once(n)));
    for (k, (s, e)) in bounds.enumerate() {
        let area: f64 = (s..e)
            .map(|i| {
                let (p, q) = (at(i), at(if i + 1 == e { s } else { i + 1 }));
                p[0] * q[1] - q[0] * p[1]
            })
            .sum::<f64>()
            .abs();
        scale += area;
        want += if k == 0 { area } else { -area };
    }
    let (mut signed, mut total) = (0.0, 0.0);
    for t in triangles.as_chunks::<3>().0 {
        let area = area2(t.map(at));
        signed += area;
        total += area.abs();
    }
    let slack = 1e-8 * scale;
    (signed.abs() - want).abs() <= slack && total - signed.abs() <= slack
}

/// Twice the signed area of a triangle in the plane basis.
fn area2(p: [[f64; 2]; 3]) -> f64 {
    (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1])
}

fn key(a: u32, b: u32) -> Edge {
    (a.min(b), a.max(b))
}

/// How many times each edge of one polygon (its loops `members`) and its cap is used: once per
/// outline edge for its side triangle, and once per cap triangle on it. A closed fill uses every
/// edge twice.
fn edge_uses(rings: &[Ring], members: &[usize], cap: &[[u32; 3]]) -> IdMap<Edge, u32> {
    let edges = members.iter().map(|&r| rings[r].ids.len()).sum::<usize>() + cap.len() * 3;
    let mut uses: IdMap<Edge, u32> = IdMap::with_capacity_and_hasher(edges, Default::default());
    for e in members.iter().flat_map(|&r| rings[r].edges()) {
        *uses.entry(e).or_default() += 1;
    }
    for t in cap {
        for k in 0..3 {
            *uses.entry(key(t[k], t[(k + 1) % 3])).or_default() += 1;
        }
    }
    uses
}

/// Adds `by` to an edge's use count, dropping the edge once nothing uses it.
fn bump(uses: &mut IdMap<Edge, u32>, e: Edge, by: i32) {
    let n = uses.entry(e).or_default();
    *n = n.saturating_add_signed(by);
    if *n == 0 {
        uses.remove(&e);
    }
}

/// Earcut can cut off a triangle with no area, one corner on the edge between the other two, where
/// the outline has points in a line: such a triangle doubles edges that other triangles cover too,
/// or stands in for a real one beside it. It is dropped, and the cap triangle beyond its long edge,
/// if any, is left with that corner on its edge for `mend_t_junctions` to fan. Flat here means
/// flat to f64 rounding: a thin triangle among f32-rounded points is a real part of the cap. One
/// whose long edge is an outline edge stays, since where the outline touches itself it is the cap
/// of that edge.
fn drop_flat(
    cap: &mut Vec<[u32; 3]>,
    uses: &mut IdMap<Edge, u32>,
    rings: &[Ring],
    members: &[usize],
    flat: &dyn Fn(u32) -> [f64; 2],
) {
    // The corner of t, if any, strictly between the other two and on their line.
    let corner = |t: [u32; 3]| {
        let p = t.map(flat);
        (0..3).find(|&k| {
            let (a, b, c) = (p[(k + 1) % 3], p[(k + 2) % 3], p[k]);
            let d = [b[0] - a[0], b[1] - a[1]];
            let r = [c[0] - a[0], c[1] - a[1]];
            let length2 = d[0] * d[0] + d[1] * d[1];
            let along = (r[0] * d[0] + r[1] * d[1]) / length2;
            along > 0.0 && along < 1.0 && (d[0] * r[1] - d[1] * r[0]).abs() <= 1e-9 * length2
        })
    };
    let found: Vec<(usize, usize)> = (0..cap.len())
        .filter_map(|n| corner(cap[n]).map(|k| (n, k)))
        .collect();
    if found.is_empty() {
        return;
    }
    let outline: IdSet<Edge> = members.iter().flat_map(|&r| rings[r].edges()).collect();
    let mut keep = vec![true; cap.len()];
    for (n, k) in found {
        let t = cap[n];
        if outline.contains(&key(t[(k + 1) % 3], t[(k + 2) % 3])) {
            continue;
        }
        keep[n] = false;
        for j in 0..3 {
            bump(uses, key(t[j], t[(j + 1) % 3]), -1);
        }
    }
    let mut kept = keep.iter();
    cap.retain(|_| *kept.next().unwrap_or(&true));
}

/// Earcut leaves out the outline points `simplify` dropped, may drop more where the outline runs
/// straight, and may bridge a hole along a line the outline also runs on. A cap triangle then
/// spans points the side's triangles still end at: a T-junction, open edges.
///
/// Every edge must be used twice (an outline edge by its side triangle and a cap triangle, an
/// inner edge by two cap triangles). Each cap edge a-b used once is split at the outline vertices
/// of the polygon (its loops `members`) that lie on it, strictly between a and b: its triangle
/// becomes a fan from the opposite corner c through those points, which keeps the winding. With
/// `near`, the fast fill's tolerance, those points are the chain of other once-used edges that
/// runs along the segment from a to b within `near` of it (`chain_along`); only where there is
/// none (a cap edge that partly covers another) is it every such vertex the grid finds near the
/// segment, which a cluster of points closer than `near` could confuse. Without, as for the plain
/// fill, only a chain on the segment to f64 rounding counts (`chain_exact`). `uses` (see
/// `edge_uses`) is kept up to date.
fn mend_t_junctions(
    cap: &mut Vec<[u32; 3]>,
    uses: &mut IdMap<Edge, u32>,
    rings: &[Ring],
    members: &[usize],
    flat: &dyn Fn(u32) -> [f64; 2],
    near: Option<f64>,
) {
    let mut once_at = OnceEdges::new(uses);
    if once_at.is_empty() {
        return;
    }
    // Only for cap edges with no chain along them, so built the first time one turns up.
    let mut index: Option<PointIndex> = None;
    // Each split uses up points; this only stops a cycle that should not happen.
    let points: usize = members.iter().map(|&r| rings[r].ids.len()).sum();
    let mut budget = 4 * points + 16;
    let mut work: Vec<usize> = (0..cap.len()).collect();
    let (mut inside, mut scratch) = (Vec::new(), Vec::new());
    while let Some(n) = work.pop() {
        let t = cap[n];
        for k in 0..3 {
            let (a, b, c) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
            if uses.get(&key(a, b)) != Some(&1) {
                continue;
            }
            let chain = Chain {
                ends: ((a, flat(a)), (b, flat(b))),
                apex: c,
                once_at: &once_at,
                uses,
                flat,
            };
            match near {
                Some(near) => {
                    if !chain_along(&chain, near, &mut inside, &mut scratch) {
                        index
                            .get_or_insert_with(|| PointIndex::new(rings, members, near))
                            .on_segment(chain.ends.0, chain.ends.1, c, near, &mut inside);
                    }
                }
                None => chain_exact(&chain, &mut inside),
            }
            if inside.is_empty() {
                continue;
            }
            budget = budget.saturating_sub(inside.len());
            // a, inside..., b replaces the edge a-b; fan the triangle from c.
            let points: Vec<u32> = once(a)
                .chain(inside.iter().map(|&(_, v)| v))
                .chain(once(b))
                .collect();
            bump(uses, key(a, b), -1);
            for pair in points.windows(2) {
                let e = key(pair[0], pair[1]);
                bump(uses, e, 1);
                if uses.get(&e) == Some(&1) {
                    once_at.add(e);
                }
            }
            for &(_, p) in &inside {
                bump(uses, key(p, c), 2);
            }
            cap[n] = [points[0], points[1], c];
            for pair in points[1..].windows(2) {
                cap.push([pair[0], pair[1], c]);
            }
            // The fan's outer triangles keep the other two edges; they may need mending too.
            work.push(n);
            work.push(cap.len() - 1);
            break;
        }
        if budget == 0 {
            break;
        }
    }
}

/// The other ends of the edges used once at each vertex, for `chain_along`: the ones at the start
/// in a sorted list, the few that come up later in a map.
struct OnceEdges {
    pairs: Vec<(u32, u32)>,
    later: IdMap<u32, Vec<u32>>,
}

impl OnceEdges {
    fn new(uses: &IdMap<Edge, u32>) -> OnceEdges {
        let mut pairs: Vec<(u32, u32)> = uses
            .iter()
            .filter(|&(_, &n)| n == 1)
            .flat_map(|(&(a, b), _)| [(a, b), (b, a)])
            .collect();
        pairs.sort_unstable();
        OnceEdges {
            pairs,
            later: IdMap::default(),
        }
    }

    fn is_empty(&self) -> bool {
        self.pairs.is_empty() && self.later.is_empty()
    }

    fn add(&mut self, (a, b): Edge) {
        self.later.entry(a).or_default().push(b);
        self.later.entry(b).or_default().push(a);
    }

    /// The other ends at v; some edges may no longer be used once.
    fn at(&self, v: u32) -> impl Iterator<Item = u32> + '_ {
        let first = self.pairs.partition_point(|p| p.0 < v);
        let count = self.pairs[first..].partition_point(|p| p.0 == v);
        let later = self.later.get(&v).map_or(&[][..], Vec::as_slice);
        self.pairs[first..first + count]
            .iter()
            .map(|p| p.1)
            .chain(later.iter().copied())
    }
}

/// A cap edge a-b to mend (`ends`, with their points), the opposite corner of its triangle, and
/// the edges used once that a chain from a to b may follow.
struct Chain<'a> {
    ends: ((u32, [f64; 2]), (u32, [f64; 2])),
    apex: u32,
    once_at: &'a OnceEdges,
    uses: &'a IdMap<Edge, u32>,
    flat: &'a dyn Fn(u32) -> [f64; 2],
}

/// Into `out`, as (fraction along, id): the points p1..pk of a chain a, p1, ..., pk, b of edges
/// used once (other than a-b itself, and not through the triangle's opposite corner) that runs
/// along segment a-b, each point within `tol` of it (plus f64 rounding) and no nearer a than the
/// last. False when there is no such chain. `scratch` is room for the search, kept between calls.
///
/// Points that lie closer together than `tol` (a cut through a vertex leaves crossing points a few
/// f32 steps apart) can each look as if they were on the segment, so the search goes depth first,
/// nearest point first, backs up out of dead ends, and stops at the first point with an edge used
/// once to b. A point level with the last one (crossing points that rounded to the same spot, as
/// along a fine cylinder far from the origin) may follow it, but never the one before it.
fn chain_along(
    chain: &Chain,
    tol: f64,
    out: &mut Vec<(f64, u32)>,
    scratch: &mut Vec<(f64, u32)>,
) -> bool {
    let ((a, pa), (b, pb)) = chain.ends;
    out.clear();
    scratch.clear();
    let d = [pb[0] - pa[0], pb[1] - pa[1]];
    let length2 = d[0] * d[0] + d[1] * d[1];
    if !(length2 > 0.0 && length2.is_finite()) {
        return false;
    }
    let length = length2.sqrt();
    let off = (tol + 1e-9 * length) * length;
    // Puts the next points from v, reached `along` the segment from `from`, on `scratch`, the
    // nearest last; false when an edge used once runs on from v to b.
    let step = |v: u32, from: u32, along: f64, scratch: &mut Vec<(f64, u32)>| {
        let start = scratch.len();
        for x in chain.once_at.at(v) {
            let skip = x == from || x == chain.apex || (v == a && x == b);
            if skip || chain.uses.get(&key(v, x)) != Some(&1) {
                continue;
            }
            if x == b {
                scratch.truncate(start);
                return false;
            }
            let p = (chain.flat)(x);
            let r = [p[0] - pa[0], p[1] - pa[1]];
            let t = (r[0] * d[0] + r[1] * d[1]) / length2;
            if t >= along && t < 1.0 && (d[0] * r[1] - d[1] * r[0]).abs() <= off {
                scratch.push((t, x));
            }
        }
        scratch[start..].sort_unstable_by(|x, y| y.0.total_cmp(&x.0));
        true
    };
    if !step(a, a, 0.0, scratch) {
        // a-b's only neighbour on the line is another edge a-b: nothing to put in between.
        return false;
    }
    // Where each point's next points start on `scratch`, from a's on.
    let mut frames = vec![0];
    let mut budget = 64 + 2 * chain.once_at.pairs.len();
    while let Some(&start) = frames.last() {
        if scratch.len() == start {
            frames.pop();
            out.pop();
            continue;
        }
        let Some((t, x)) = scratch.pop() else {
            break;
        };
        budget -= 1;
        if budget == 0 {
            break;
        }
        let from = out.last().map_or(a, |&(_, v)| v);
        out.push((t, x));
        let next = scratch.len();
        if !step(x, from, t, scratch) {
            return true;
        }
        frames.push(next);
    }
    out.clear();
    false
}

/// The plain fill's search, as `chain_along` but on the segment to f64 rounding only, and greedy:
/// from a, always on to the nearest next point that an edge used once reaches, until b. Leaves
/// `out` empty when there is no such chain.
fn chain_exact(chain: &Chain, out: &mut Vec<(f64, u32)>) {
    let ((a, pa), (b, pb)) = chain.ends;
    out.clear();
    let d = [pb[0] - pa[0], pb[1] - pa[1]];
    let length2 = d[0] * d[0] + d[1] * d[1];
    if !(length2 > 0.0 && length2.is_finite()) {
        return;
    }
    let (mut current, mut along) = (a, 0.0);
    loop {
        let mut best: Option<(f64, u32)> = None;
        for x in chain.once_at.at(current) {
            if (current == a && x == b)
                || x == chain.apex
                || chain.uses.get(&key(current, x)) != Some(&1)
            {
                continue;
            }
            let p = (chain.flat)(x);
            let r = [p[0] - pa[0], p[1] - pa[1]];
            let t = (r[0] * d[0] + r[1] * d[1]) / length2;
            // Off the line by more than f64 noise: some other edge, not part of this chain.
            let off = (d[0] * r[1] - d[1] * r[0]).abs();
            if t <= along || t > 1.0 || off > 1e-9 * length2 {
                continue;
            }
            if best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, x));
            }
        }
        let Some((t, x)) = best else {
            out.clear();
            return;
        };
        if x == b {
            return;
        }
        if out.len() > chain.once_at.pairs.len() {
            out.clear();
            return;
        }
        out.push((t, x));
        current = x;
        along = t;
    }
}

/// A uniform grid over the plane, about one cell per point, for the outline searches.
struct Grid {
    min: [f64; 2],
    cell: f64,
    size: [usize; 2],
}

impl Grid {
    /// Covers `points`, with cells at least `least` wide.
    fn new(points: impl Iterator<Item = [f64; 2]>, least: f64) -> Grid {
        let (mut min, mut max, mut count) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2], 0usize);
        for p in points {
            for k in 0..2 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
            count += 1;
        }
        let extent = [max[0] - min[0], max[1] - min[1]];
        if !(extent[0].is_finite() && extent[1].is_finite()) {
            // No points, or coordinates that are not numbers: one cell holds everything.
            return Grid {
                min: [0.0; 2],
                cell: f64::INFINITY,
                size: [1, 1],
            };
        }
        let count = count.max(1);
        let mut cell = (extent[0] * extent[1] / count as f64).sqrt();
        // All the points on a line, or on one point.
        if cell.is_nan() || cell <= 0.0 {
            cell = (extent[0] + extent[1]) / count as f64;
        }
        cell = cell.max(least);
        if cell.is_nan() || cell <= 0.0 {
            cell = 1.0;
        }
        let dims = |cell: f64| extent.map(|e| ((e / cell) as usize).saturating_add(1));
        let mut size = dims(cell);
        while size[0].saturating_mul(size[1]) > 4 * count + 64 {
            cell *= 2.0;
            size = dims(cell);
        }
        Grid { min, cell, size }
    }

    /// The same area in cells twice as wide.
    fn coarser(&self) -> Grid {
        Grid {
            min: self.min,
            cell: self.cell * 2.0,
            size: self.size.map(|s| s.div_ceil(2)),
        }
    }

    fn len(&self) -> usize {
        self.size[0] * self.size[1]
    }

    fn cell_of(&self, p: [f64; 2]) -> [usize; 2] {
        [0, 1].map(|k| (((p[k] - self.min[k]) / self.cell) as usize).min(self.size[k] - 1))
    }

    fn index(&self, c: [usize; 2]) -> usize {
        c[1] * self.size[0] + c[0]
    }

    /// Calls `each` with every cell within one cell of segment a-b, some of them more than once.
    fn along(&self, a: [f64; 2], b: [f64; 2], mut each: impl FnMut(usize)) {
        let d = [b[0] - a[0], b[1] - a[1]];
        // A sample every half cell: every point of the segment is then in a sample's cell or in
        // one next to it.
        let steps = ((d[0].hypot(d[1]) / self.cell * 2.0).ceil() as usize)
            .min(2 * (self.size[0] + self.size[1]))
            + 1;
        for s in 0..=steps {
            let f = s as f64 / steps as f64;
            let c = self.cell_of([a[0] + d[0] * f, a[1] + d[1] * f]);
            for y in c[1].saturating_sub(1)..=(c[1] + 1).min(self.size[1] - 1) {
                for x in c[0].saturating_sub(1)..=(c[0] + 1).min(self.size[0] - 1) {
                    each(self.index([x, y]));
                }
            }
        }
    }
}

/// The outline vertices of one polygon in a grid, for finding the ones on a cap edge.
struct PointIndex {
    grid: Grid,
    start: Vec<usize>,
    /// (vertex id, point), grouped by cell.
    entries: Vec<(u32, [f64; 2])>,
    /// The query that last looked at each cell.
    seen: Vec<u32>,
    query: u32,
}

impl PointIndex {
    fn new(rings: &[Ring], members: &[usize], tol: f64) -> PointIndex {
        let entries: Vec<(u32, [f64; 2])> = members
            .iter()
            .flat_map(|&r| {
                rings[r]
                    .ids
                    .iter()
                    .copied()
                    .zip(rings[r].points.iter().copied())
            })
            .collect();
        // Cells wide enough that a point within tol of a segment is next to a cell it passes.
        let grid = Grid::new(entries.iter().map(|e| e.1), 4.0 * tol);
        let cells: Vec<usize> = entries
            .iter()
            .map(|e| grid.index(grid.cell_of(e.1)))
            .collect();
        let mut start = vec![0usize; grid.len() + 1];
        for &c in &cells {
            start[c + 1] += 1;
        }
        for c in 0..grid.len() {
            start[c + 1] += start[c];
        }
        let mut fill = start.clone();
        let mut sorted = vec![(0, [0.0; 2]); entries.len()];
        for (e, &c) in entries.iter().zip(&cells) {
            sorted[fill[c]] = *e;
            fill[c] += 1;
        }
        PointIndex {
            seen: vec![0; grid.len()],
            grid,
            start,
            entries: sorted,
            query: 0,
        }
    }

    /// Into `out`, as (fraction along, id) ordered from `a`: the vertices other than `a`, `b` and
    /// `skip` that lie strictly between a and b and within `tol` of the segment.
    fn on_segment(
        &mut self,
        a: (u32, [f64; 2]),
        b: (u32, [f64; 2]),
        skip: u32,
        tol: f64,
        out: &mut Vec<(f64, u32)>,
    ) {
        out.clear();
        let (pa, pb) = (a.1, b.1);
        let d = [pb[0] - pa[0], pb[1] - pa[1]];
        let length2 = d[0] * d[0] + d[1] * d[1];
        if !(length2 > 0.0 && length2.is_finite()) {
            return;
        }
        let length = length2.sqrt();
        self.query = self.query.wrapping_add(1);
        if self.query == 0 {
            self.seen.fill(0);
            self.query = 1;
        }
        let PointIndex {
            grid,
            start,
            entries,
            seen,
            query,
        } = self;
        grid.along(pa, pb, |c| {
            if seen[c] == *query {
                return;
            }
            seen[c] = *query;
            for &(v, q) in &entries[start[c]..start[c + 1]] {
                if v == a.0 || v == b.0 || v == skip {
                    continue;
                }
                let r = [q[0] - pa[0], q[1] - pa[1]];
                let t = (r[0] * d[0] + r[1] * d[1]) / length2;
                if t > 0.0 && t < 1.0 && (d[0] * r[1] - d[1] * r[0]).abs() <= tol * length {
                    out.push((t, v));
                }
            }
        });
        out.sort_unstable_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
        out.dedup_by_key(|x| x.1);
    }
}

/// An inner cap edge must be new. Where the part touches the plane along one of its own edges
/// from one side (the floor of a groove, a saddle the plane is tangent to), that edge can run
/// right across the outline, and earcut may pick it as a diagonal or a hole bridge: the piece on
/// that side would then have four triangles on the edge. Each such edge becomes the other diagonal
/// of its two triangles' quad when that quad is convex. Otherwise a new vertex m in its middle
/// splits both triangles, so the cap runs a-m-b beside the part's edge instead of along it, which
/// is just as watertight. Returns false when some such edge could not be moved because its two
/// triangles run it the same way, which a consistent fill never does.
fn avoid_part_edges(
    cap: &mut Vec<[u32; 3]>,
    uses: &mut IdMap<Edge, u32>,
    touching: &IdSet<Edge>,
    flat: &dyn Fn(u32) -> [f64; 2],
    positions: &[f32],
    points: &mut Vec<[f32; 3]>,
) -> bool {
    let base = positions.len() / 3;
    let old = |v: u32| (v as usize) < base;
    let mut moved_all = true;
    // A triangle with two such edges has the second one moved on a later pass.
    for _ in 0..4 {
        // (triangle, corner) for both triangles of each inner cap edge that repeats a part edge.
        let mut clashes: IdMap<Edge, Vec<(usize, usize)>> = IdMap::default();
        for (n, t) in cap.iter().enumerate() {
            for k in 0..3 {
                let e = key(t[k], t[(k + 1) % 3]);
                if touching.contains(&e) && uses.get(&e) == Some(&2) {
                    clashes.entry(e).or_default().push((n, k));
                }
            }
        }
        let mut clashes: Vec<_> = clashes.into_values().filter(|s| s.len() == 2).collect();
        clashes.sort_unstable();
        let mut touched = IdSet::default();
        let mut deferred = false;
        for slots in clashes {
            let [(n1, k1), (n2, k2)] = [slots[0], slots[1]];
            if touched.contains(&n1) || touched.contains(&n2) {
                deferred = true;
                continue;
            }
            let corner = |n: usize, k: usize| cap[n][k % 3];
            let (a, b, c) = (corner(n1, k1), corner(n1, k1 + 1), corner(n1, k1 + 2));
            let d = corner(n2, k2 + 2);
            if corner(n2, k2) != b || corner(n2, k2 + 1) != a || c == d || !(old(a) && old(b)) {
                moved_all = false;
                continue;
            }
            touched.insert(n1);
            touched.insert(n2);
            bump(uses, key(a, b), -2);
            // The other diagonal must be new too and cross the old one.
            let flipped = key(c, d);
            let convex = old(c) && old(d) && {
                let crosses = |p: [[f64; 2]; 3], q: [[f64; 2]; 3]| area2(p) * area2(q) < 0.0;
                let [pa, pb, pc, pd] = [a, b, c, d].map(flat);
                crosses([pc, pd, pa], [pc, pd, pb]) && crosses([pa, pb, pc], [pa, pb, pd])
            };
            if convex && !touching.contains(&flipped) && !uses.contains_key(&flipped) {
                cap[n1] = [a, d, c];
                cap[n2] = [d, b, c];
                bump(uses, flipped, 2);
                continue;
            }
            let m = (base + points.len()) as u32;
            let (i, j) = (a as usize * 3, b as usize * 3);
            points.push(
                [0, 1, 2]
                    .map(|k| ((positions[i + k] as f64 + positions[j + k] as f64) / 2.0) as f32),
            );
            cap[n1] = [a, m, c];
            cap.push([m, b, c]);
            cap[n2] = [b, m, d];
            cap.push([m, a, d]);
            for e in [key(a, m), key(m, b), key(m, c), key(m, d)] {
                bump(uses, e, 2);
            }
        }
        if !deferred {
            return moved_all;
        }
    }
    false
}

/// Cuts a polygon with many holes into simple pieces for earcut, which is quadratic on such a
/// polygon twice over: it links each hole to the outline by scanning everything linked so far,
/// and once linked, nearly every vertex is reflex, so each triangle it cuts off costs a walk round
/// the whole outline (a plate with 10,000 holes took 5 s, 22,500 holes 40 s).
///
/// Each hole is linked to the outline on its left the way earcut does it, but the link is found
/// through a grid of the outline's edges. Then each hole is linked once more on its right, by the
/// same search on the outline turned half round: each such link is a diagonal that splits a
/// piece in two, so the pieces come out about one per hole. In a row of holes each one's left
/// link already runs to the next, and the right link found is the same one again; it is then
/// made from the hole's other point level with the first (the other corner of a square hole's
/// side), which closes off the gap between the two. Holes in a row with no such point (round
/// ones) keep the row on two long pieces, which earcut takes in time quadratic in the row's
/// length; `fill_polygon` still checks the result. Takes earcut's input (`coords`, `starts`) and
/// returns the pieces as rings of point indices, or None when some hole found no link on its
/// left, or when the links' grid scans used up BRIDGE_WORK grid entries per point: earcut then
/// gets the whole polygon, as it would without this, so the linking never costs more than a
/// bounded amount on top. The budget matters on parts turned off the axes: their straightened
/// outline is a few long edges, and a link's blocking-point scan reads every cell those cross (a
/// turned column of 3,000 holes took a minute; with the budget 0.5 s, the pre-fix fill 0.9 s).
/// Measured limit: a turned perforated plate cut parallel to its faces still falls back to earcut
/// on the whole polygon, about 14 s for 10,000 holes (the pre-fix fill took 6 to 8 s); plates that
/// lie along the axes take 0.4 s for 22,500 holes.
fn bridge_holes(coords: &[f64], starts: &[usize]) -> Option<Vec<Vec<usize>>> {
    let n = coords.len() / 2;
    let budget = Cell::new(BRIDGE_WORK * n + BRIDGE_WORK_BASE);
    let at = |i: usize| [coords[2 * i], coords[2 * i + 1]];
    let bounds: Vec<(usize, usize)> = once(0)
        .chain(starts.iter().copied())
        .zip(starts.iter().copied().chain(once(n)))
        .collect();
    let mut links = Links {
        point: (0..n).collect(),
        pos: (0..n).map(at).collect(),
        next: vec![0; n],
        prev: vec![0; n],
        ring: vec![0; n],
        twin: (0..n).collect(),
        budget,
    };
    // The outer loop counter-clockwise, the holes clockwise, as earcut has them.
    for (r, &(s, e)) in bounds.iter().enumerate() {
        if e < s + 3 {
            return None;
        }
        let area: f64 = (s..e)
            .map(|i| {
                let (p, q) = (at(i), at(if i + 1 == e { s } else { i + 1 }));
                p[0] * q[1] - q[0] * p[1]
            })
            .sum();
        let forward = (area > 0.0) == (r == 0);
        for i in s..e {
            let after = if i + 1 == e { s } else { i + 1 };
            let before = if i == s { e - 1 } else { i - 1 };
            (links.next[i], links.prev[i]) = if forward {
                (after, before)
            } else {
                (before, after)
            };
            links.ring[i] = r;
        }
    }
    // Each hole by its leftmost point, then by its rightmost, as earcut would take them.
    // Per hole, its points furthest that way: the first one as earcut would pick it, then the
    // others level with it (the other corner of a square hole's side).
    let hole_ends = |sign: f64| -> Vec<Vec<usize>> {
        let mut ends: Vec<Vec<usize>> = bounds[1..]
            .iter()
            .map(|&(s, e)| {
                let first = (s..e)
                    .min_by(|&a, &b| (sign * at(a)[0]).total_cmp(&(sign * at(b)[0])))
                    .unwrap_or(s);
                let level = at(first)[0];
                once(first)
                    .chain((s..e).filter(|&i| i != first && at(i)[0] == level))
                    .collect()
            })
            .collect();
        ends.sort_by(|a, b| (sign * at(a[0])[0]).total_cmp(&(sign * at(b[0])[0])));
        ends
    };
    let mut merged = vec![false; bounds.len()];
    merged[0] = true;
    links.link_all(&hole_ends(1.0), &mut merged, true)?;
    // The outline turned half round keeps its winding, so the same search links to the right.
    for p in &mut links.pos {
        *p = [-p[0], -p[1]];
    }
    links.link_all(&hole_ends(-1.0), &mut merged, false)?;
    let total = links.point.len();
    let mut seen = vec![false; total];
    let mut pieces = Vec::new();
    for first in 0..total {
        let mut piece = Vec::new();
        let mut p = first;
        while !seen[p] {
            seen[p] = true;
            piece.push(links.point[p]);
            p = links.next[p];
        }
        if !piece.is_empty() {
            if p != first {
                return None;
            }
            pieces.push(piece);
        }
    }
    Some(pieces)
}

/// Earcut's linked outline while `bridge_holes` links the holes in: nodes are the points, then
/// the copies each link adds.
struct Links {
    point: Vec<usize>,
    pos: Vec<[f64; 2]>,
    next: Vec<usize>,
    prev: Vec<usize>,
    /// The loop each node came from: 0 the outer one, then the holes.
    ring: Vec<usize>,
    /// The nodes of one point in a circular list.
    twin: Vec<usize>,
    /// Grid entries the link searches may still read; at 0 the linking gives up.
    budget: Cell<usize>,
}

/// Grid entries the link searches may read per point, plus a base, before `bridge_holes` gives
/// the polygon to earcut whole. Measured: hole rows and grids read 16 to 180 entries per point
/// (180 for a plate with 22,500 holes), a turned column of holes 180,000 to 1,700,000.
const BRIDGE_WORK: usize = 1024;
const BRIDGE_WORK_BASE: usize = 1 << 20;

impl Links {
    /// Links each hole of `queue` in turn to the outline on its left (`find_bridge`), from the
    /// first of its nodes listed there, and marks its loop `merged`. Fails when a link is
    /// `required` and none is found. A link that is not required and only repeats one already
    /// made (a row of holes, each already linked to the next) is tried from the hole's other
    /// nodes instead.
    fn link_all(
        &mut self,
        queue: &[Vec<usize>],
        merged: &mut [bool],
        required: bool,
    ) -> Option<()> {
        let grid = Grid::new(self.pos.iter().copied(), 0.0);
        // Each node under every cell next to its edge to the next node.
        let mut cells: Vec<Vec<u32>> = vec![Vec::new(); grid.len()];
        let mut stamp = vec![0u32; grid.len()];
        let mut round = 0u32;
        let mut list = |p: usize, links: &Links, cells: &mut Vec<Vec<u32>>| {
            round += 1;
            grid.along(links.pos[p], links.pos[links.next[p]], |c| {
                if stamp[c] != round {
                    stamp[c] = round;
                    cells[c].push(p as u32);
                }
            });
        };
        for p in 0..self.pos.len() {
            list(p, self, &mut cells);
        }
        for ends in queue {
            let tries = if required { &ends[..1] } else { &ends[..] };
            for &h in tries {
                let Some(m) = self.find_bridge(h, &grid, &cells, merged) else {
                    if required || self.budget.get() == 0 {
                        return None;
                    }
                    continue;
                };
                // Where links already meet a point, its copies split the angle there: take the
                // copy whose angle the new link runs into, at both ends.
                let m = self.toward(m, h);
                let h = self.toward(h, m);
                if self.linked(h, m) {
                    continue;
                }
                let (c, d) = self.split(m, h);
                merged[self.ring[h]] = true;
                // m's old edge now leaves from its copy c, m runs along the link to h, d back
                // along it.
                for p in [c, m, d] {
                    list(p, self, &mut cells);
                }
                break;
            }
        }
        Some(())
    }

    /// Takes `work` grid entries from the budget, or empties it and returns false when it is short.
    fn spend(&self, work: usize) -> bool {
        let left = self.budget.get();
        self.budget.set(left.saturating_sub(work.max(1)));
        left >= work.max(1)
    }

    /// The copy of node v's point that a link towards node b leaves from.
    fn toward(&self, v: usize, b: usize) -> usize {
        let mut n = v;
        loop {
            if self.locally_inside(n, b) {
                return n;
            }
            n = self.twin[n];
            if n == v {
                return v;
            }
        }
    }

    /// Whether some copy of a already has an edge to b's point.
    fn linked(&self, a: usize, b: usize) -> bool {
        let mut n = a;
        loop {
            let ends = [self.point[self.next[n]], self.point[self.prev[n]]];
            if ends.contains(&self.point[b]) {
                return true;
            }
            n = self.twin[n];
            if n == a {
                return false;
            }
        }
    }

    /// Earcut's `area`: negative where p, q, r turn counter-clockwise.
    fn turn(&self, p: usize, q: usize, r: usize) -> f64 {
        let (p, q, r) = (self.pos[p], self.pos[q], self.pos[r]);
        (q[1] - p[1]) * (r[0] - q[0]) - (q[0] - p[0]) * (r[1] - q[1])
    }

    /// Earcut's `locallyInside`: whether a diagonal from a towards b starts inside the outline.
    fn locally_inside(&self, a: usize, b: usize) -> bool {
        let (p, n) = (self.prev[a], self.next[a]);
        if self.turn(p, a, n) < 0.0 {
            self.turn(a, b, n) >= 0.0 && self.turn(a, p, b) >= 0.0
        } else {
            self.turn(a, b, p) < 0.0 || self.turn(a, n, b) < 0.0
        }
    }

    /// Earcut's `findHoleBridge`, with the grid in place of its scans: the outline node that hole
    /// node h links to. Only nodes of `merged` loops count as outline.
    fn find_bridge(
        &self,
        h: usize,
        grid: &Grid,
        cells: &[Vec<u32>],
        merged: &[bool],
    ) -> Option<usize> {
        let [hx, hy] = self.pos[h];
        // The nearest edge left of h that a ray to the left crosses going down (the outline runs
        // counter-clockwise and the merged holes clockwise, so those edges face h). Every edge it
        // crosses is listed in the cell of the crossing, so the cells are read right to left.
        let mut qx = f64::NEG_INFINITY;
        let mut m = None;
        let [cx, cy] = grid.cell_of(self.pos[h]);
        for x in (0..=cx).rev() {
            let entries = &cells[grid.index([x, cy])];
            if !self.spend(entries.len()) {
                return None;
            }
            for &p in entries {
                let p = p as usize;
                if !merged[self.ring[p]] {
                    continue;
                }
                let next = self.next[p];
                let ([px, py], [nx, ny]) = (self.pos[p], self.pos[next]);
                if !(hy <= py && hy >= ny && ny != py) {
                    continue;
                }
                let x = px + (hy - py) * (nx - px) / (ny - py);
                // h's own loop, once merged (linking to the right), meets the ray only at h.
                let own = self.ring[p] == self.ring[h] || self.ring[next] == self.ring[h];
                if x <= hx && x > qx && !(own && x == hx) {
                    qx = x;
                    if x == hx && hy == py {
                        return Some(p);
                    }
                    if x == hx && hy == ny {
                        return Some(next);
                    }
                    m = Some(if px < nx { p } else { next });
                }
            }
            // Crossings not seen yet lie in columns left of this one.
            if m.is_some() && qx >= grid.min[0] + (x + 1) as f64 * grid.cell {
                break;
            }
        }
        let m = m?;
        if qx == hx {
            // The hole touches that edge.
            return Some(self.prev[m]);
        }
        // An outline point inside the triangle of h, the crossing and m would block the link: of
        // those, take the one at the smallest angle to the ray.
        let [mx, my] = self.pos[m];
        let (x1, x2) = if hy < my { (hx, qx) } else { (qx, hx) };
        let (n1, n2) = ([x1, hy], [x2, hy]);
        let mut best = m;
        let mut tan_min = f64::MAX / 2.0;
        let lo = grid.cell_of([mx, hy.min(my)]);
        let hi = grid.cell_of([hx, hy.max(my)]);
        for y in lo[1]..=hi[1].max(lo[1]) {
            for x in lo[0]..=hi[0].max(lo[0]) {
                let entries = &cells[grid.index([x, y])];
                if !self.spend(entries.len()) {
                    return None;
                }
                for &p in entries {
                    let p = p as usize;
                    if p == m || !merged[self.ring[p]] {
                        continue;
                    }
                    let [px, py] = self.pos[p];
                    if !(hx > px && px >= mx && in_triangle(n1, [mx, my], n2, [px, py])) {
                        continue;
                    }
                    let tan = (hy - py).abs() / (hx - px);
                    if (tan < tan_min || (tan == tan_min && px > self.pos[best][0]))
                        && self.locally_inside(p, h)
                    {
                        best = p;
                        tan_min = tan;
                    }
                }
            }
        }
        Some(best)
    }

    /// Earcut's `splitPolygon`: links a to b, joining their loops (or splitting one loop in two),
    /// and returns the copies of a and b that carry the link's way back.
    fn split(&mut self, a: usize, b: usize) -> (usize, usize) {
        let (c, d) = (self.pos.len(), self.pos.len() + 1);
        for (v, copy) in [(a, c), (b, d)] {
            self.point.push(self.point[v]);
            self.pos.push(self.pos[v]);
            self.ring.push(self.ring[v]);
            self.next.push(0);
            self.prev.push(0);
            self.twin.push(self.twin[v]);
            self.twin[v] = copy;
        }
        let (an, bp) = (self.next[a], self.prev[b]);
        self.next[a] = b;
        self.prev[b] = a;
        self.next[c] = an;
        self.prev[an] = c;
        self.next[d] = c;
        self.prev[c] = d;
        self.next[bp] = d;
        self.prev[d] = bp;
        (c, d)
    }
}

/// Earcut's `pointInTriangle`: p inside or on the edges of triangle a, b, c.
fn in_triangle(a: [f64; 2], b: [f64; 2], c: [f64; 2], p: [f64; 2]) -> bool {
    (c[0] - p[0]) * (a[1] - p[1]) - (a[0] - p[0]) * (c[1] - p[1]) >= 0.0
        && (a[0] - p[0]) * (b[1] - p[1]) - (b[0] - p[0]) * (a[1] - p[1]) >= 0.0
        && (b[0] - p[0]) * (c[1] - p[1]) - (c[0] - p[0]) * (b[1] - p[1]) >= 0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{Welder, make_box, stats, weld_indexed};
    use crate::ops::concat;

    const Z: [f64; 3] = [0.0, 0.0, 1.0];

    fn plane(normal: [f64; 3], offset: f64) -> Plane {
        let l = dot(normal, normal).sqrt();
        Plane {
            normal: normal.map(|v| v / l),
            offset,
        }
    }

    /// Volumes and (open, non-manifold) edge counts of both pieces, None for a missing side.
    fn measure(p: &CutPieces) -> [Option<(f64, u64, u64)>; 2] {
        [&p.below, &p.above].map(|m| {
            m.as_ref().map(|m| {
                let s = stats(m);
                (s.volume, s.open_edges, s.non_manifold_edges)
            })
        })
    }

    fn assert_closed_halves(mesh: &Mesh, p: &Plane, want: [f64; 2]) {
        let pieces = cut(mesh, p, true);
        let got = measure(&pieces);
        for (k, (g, w)) in got.iter().zip(want).enumerate() {
            let (v, open, bad) = g.unwrap_or_else(|| panic!("piece {k} missing"));
            assert!(
                (v - w).abs() < 1e-6 * w.abs().max(1.0),
                "piece {k}: volume {v}, want {w}"
            );
            assert_eq!((open, bad), (0, 0), "piece {k}: open / non-manifold edges");
        }
        assert!(!pieces.open_outline);
    }

    /// Closed mesh of the outer faces of the filled cells of a grid (cells `size` mm, `n` per
    /// axis), facing outwards and welded.
    fn voxels(n: [i32; 3], size: f32, filled: impl Fn(i32, i32, i32) -> bool) -> Mesh {
        let is =
            |c: [i32; 3]| (0..3).all(|k| (0..n[k]).contains(&c[k])) && filled(c[0], c[1], c[2]);
        let mut w = Welder::new(0);
        for x in 0..n[0] {
            for y in 0..n[1] {
                for z in 0..n[2] {
                    let c = [x, y, z];
                    if !is(c) {
                        continue;
                    }
                    for k in 0..3 {
                        for s in [-1, 1] {
                            let mut nb = c;
                            nb[k] += s;
                            if is(nb) {
                                continue;
                            }
                            // Corners (0,0) (1,0) (1,1) (0,1) over the next two axes face +k.
                            let (i, j) = ((k + 1) % 3, (k + 2) % 3);
                            let corner = |di: i32, dj: i32| {
                                let mut p = c;
                                p[k] += (s > 0) as i32;
                                p[i] += di;
                                p[j] += dj;
                                p.map(|v| v as f32 * size)
                            };
                            let mut quad = [corner(0, 0), corner(1, 0), corner(1, 1), corner(0, 1)];
                            if s < 0 {
                                quad.reverse();
                            }
                            w.triangle(quad[0], quad[1], quad[2]);
                            w.triangle(quad[0], quad[2], quad[3]);
                        }
                    }
                }
            }
        }
        w.finish().unwrap()
    }

    #[test]
    fn voxel_helper_builds_closed_solids() {
        let frame = voxels([3, 3, 1], 10.0, |x, y, _| (x, y) != (1, 1));
        let s = stats(&frame);
        assert_eq!(
            (s.volume, s.open_edges, s.non_manifold_edges, s.shells),
            (8000.0, 0, 0, 1)
        );
    }

    #[test]
    fn cube_at_half_height_gives_two_closed_halves() {
        let cube = make_box([0.0; 3], [20.0; 3]);
        assert_closed_halves(&cube, &plane(Z, 10.0), [4000.0, 4000.0]);
        // Tilted, through two opposite edges' midpoints.
        assert_closed_halves(
            &cube,
            &plane([1.0, 0.0, 1.0], 20.0 / 2f64.sqrt()),
            [4000.0, 4000.0],
        );
    }

    #[test]
    fn plane_through_the_cube_faces_keeps_it_whole() {
        let cube = make_box([0.0; 3], [20.0; 3]);
        for (offset, below) in [(0.0, false), (20.0, true), (-5.0, false), (25.0, true)] {
            let p = cut(&cube, &plane(Z, offset), true);
            let whole = if below { &p.below } else { &p.above };
            assert_eq!(whole.as_ref(), Some(&cube), "z = {offset}");
            let other = if below { &p.above } else { &p.below };
            assert!(other.is_none(), "z = {offset}");
            assert!(!p.open_outline);
        }
    }

    #[test]
    fn diagonal_plane_through_cube_vertices() {
        // x = y holds four cube vertices and two face diagonals, no edge crosses strictly.
        let cube = make_box([0.0; 3], [20.0; 3]);
        assert_closed_halves(&cube, &plane([1.0, -1.0, 0.0], 0.0), [4000.0, 4000.0]);
        // x + y + z = 20 runs through three vertices: a corner tetrahedron and the rest.
        let corner = 20.0f64.powi(3) / 6.0;
        assert_closed_halves(
            &cube,
            &plane([1.0, 1.0, 1.0], 20.0 / 3f64.sqrt()),
            [corner, 8000.0 - corner],
        );
    }

    #[test]
    fn edge_in_the_plane_and_vertex_touching_it() {
        // Tetrahedron with edge AB on z = 0, C above and D below.
        let mesh = Mesh {
            positions: vec![
                0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 5.0, 10.0, 5.0, 5.0, 10.0, -5.0,
            ],
            indices: vec![0, 1, 2, 0, 3, 1, 0, 2, 3, 1, 3, 2],
        };
        let v = stats(&mesh).volume;
        assert!(v > 0.0);
        assert_closed_halves(&mesh, &plane(Z, 0.0), [v / 2.0, v / 2.0]);
        // A plane through D alone only touches it.
        let p = cut(&mesh, &plane(Z, -5.0), true);
        assert!(p.below.is_none());
        assert_eq!(p.above.as_ref(), Some(&mesh));
    }

    #[test]
    fn vertex_row_on_the_plane_is_capped() {
        // A 20 mm cube whose side faces have a row of vertices at z = 10 (two cells stacked).
        let stacked = voxels([1, 1, 2], 10.0, |_, _, _| true);
        let stacked = Mesh {
            positions: stacked.positions.iter().map(|v| v * 2.0).collect(),
            ..stacked
        };
        // 20 × 20 × 40 now; cells end at z = 20.
        assert_closed_halves(&stacked, &plane(Z, 20.0), [8000.0, 8000.0]);
    }

    #[test]
    fn stacked_boxes_with_a_shared_inner_face_fall_apart_cleanly() {
        // Two boxes welded into one mesh: the faces between them are doubled (non-manifold).
        let lower = make_box([0.0; 3], [20.0, 20.0, 10.0]);
        let upper = make_box([0.0, 0.0, 10.0], [20.0; 3]);
        let both = concat(&[&lower, &upper]).unwrap();
        let welded = weld_indexed(&both.positions, &both.indices).unwrap();
        assert!(stats(&welded).non_manifold_edges > 0);
        assert_closed_halves(&welded, &plane(Z, 10.0), [4000.0, 4000.0]);
    }

    #[test]
    fn coplanar_faces_go_with_the_solid_they_close() {
        // L shape: a step whose top (facing +z) lies in the plane.
        let step = voxels([2, 1, 2], 10.0, |x, _, z| !(x == 1 && z == 1));
        assert_closed_halves(&step, &plane(Z, 10.0), [2000.0, 1000.0]);
        // Upside-down L: an overhang whose underside (facing -z) lies in the plane.
        let overhang = voxels([2, 1, 2], 10.0, |x, _, z| !(x == 1 && z == 0));
        assert_closed_halves(&overhang, &plane(Z, 10.0), [1000.0, 2000.0]);
    }

    #[test]
    fn through_hole_gives_an_annulus_cap() {
        let frame = voxels([3, 3, 1], 10.0, |x, y, _| (x, y) != (1, 1));
        assert_closed_halves(&frame, &plane(Z, 5.0), [4000.0, 4000.0]);
        // Across the hole: two separate outlines per side.
        assert_closed_halves(&frame, &plane([1.0, 0.0, 0.0], 15.0), [4000.0, 4000.0]);
        // Tilted through the hole, leaving through the top and bottom faces too.
        let normal = plane([0.3, 0.2, 1.0], 0.0).normal;
        let tilted = Plane {
            normal,
            offset: dot(normal, [15.0, 15.0, 5.0]),
        };
        let pieces = cut(&frame, &tilted, true);
        assert!(!pieces.open_outline);
        let [b, a] = measure(&pieces);
        let (b, a) = (b.unwrap(), a.unwrap());
        assert_eq!((b.1, b.2, a.1, a.2), (0, 0, 0, 0));
        assert!((b.0 + a.0 - 8000.0).abs() < 1e-6 * 8000.0);
    }

    #[test]
    fn nested_islands_alternate_between_fill_and_hole() {
        // A hollow 5 mm cube with a 1 mm cube floating in its cavity.
        let nested = voxels([5, 5, 5], 1.0, |x, y, z| {
            [x, y, z].iter().any(|&c| c == 0 || c == 4) || (x, y, z) == (2, 2, 2)
        });
        let total = 125.0 - 27.0 + 1.0;
        assert_eq!(stats(&nested).volume, total);
        // Through the island: outer square, cavity hole, island square.
        let p = plane(Z, 2.5);
        let pieces = cut(&nested, &p, true);
        let [b, a] = measure(&pieces);
        let (b, a) = (b.unwrap(), a.unwrap());
        assert_eq!((b.1, b.2, a.1, a.2), (0, 0, 0, 0));
        assert!((b.0 - total / 2.0).abs() < 1e-9 && (a.0 - total / 2.0).abs() < 1e-9);
        // Through the island's bottom face and a vertex row of the walls.
        let pieces = cut(&nested, &plane(Z, 2.0), true);
        let [b, a] = measure(&pieces);
        let (b, a) = (b.unwrap(), a.unwrap());
        assert_eq!((b.1, b.2, a.1, a.2), (0, 0, 0, 0));
        assert!((b.0 + a.0 - total).abs() < 1e-9);
        assert!((b.0 - (25.0 + 16.0)).abs() < 1e-9, "below {}", b.0);
    }

    #[test]
    fn open_part_leaves_open_chains_uncapped() {
        // Box without its top, cut vertically: the outline is a U that does not close.
        let mut open_box = make_box([0.0; 3], [20.0; 3]);
        open_box.indices.drain(6..12);
        let pieces = cut(&open_box, &plane([1.0, 0.0, 0.0], 10.0), true);
        assert!(pieces.open_outline);
        let [b, a] = measure(&pieces);
        assert!(b.unwrap().1 > 0 && a.unwrap().1 > 0);
        // Cut horizontally the outline closes: the lower piece is a closed box, the upper one
        // keeps the open top.
        let pieces = cut(&open_box, &plane(Z, 10.0), true);
        assert!(!pieces.open_outline);
        let [b, a] = measure(&pieces);
        assert_eq!(b.unwrap().1, 0);
        assert_eq!(a.unwrap().1, 4);
        // The open rim lying in the plane is not a cut: nothing is capped.
        let pieces = cut(&open_box, &plane(Z, 20.0), true);
        assert_eq!(pieces.below.as_ref(), Some(&open_box));
        assert!(pieces.above.is_none());
    }

    #[test]
    fn without_cap_both_pieces_stay_open() {
        let cube = make_box([0.0; 3], [20.0; 3]);
        let pieces = cut(&cube, &plane(Z, 10.0), false);
        let [b, a] = measure(&pieces);
        assert_eq!(b.unwrap().1, 8);
        assert_eq!(a.unwrap().1, 8);
        assert!(!pieces.open_outline);
    }

    #[test]
    fn inverted_winding_still_caps() {
        // An inside-out cube: the caps follow the triangles, so the pieces stay closed with
        // negative volume.
        let mut cube = make_box([0.0; 3], [20.0; 3]);
        for t in cube.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
        assert_closed_halves(&cube, &plane(Z, 7.0), [-2800.0, -5200.0]);
    }

    #[test]
    fn long_straight_outline_runs_are_mended() {
        // A box whose side faces are 50 thin strips each: the cut outline has 50 points on every
        // side in a straight line, which earcut drops.
        let strips = voxels([50, 50, 1], 0.4, |_, _, _| true);
        let s = stats(&strips);
        assert_eq!((s.open_edges, s.non_manifold_edges), (0, 0));
        let v = s.volume;
        assert_closed_halves(&strips, &plane(Z, 0.2), [v / 2.0, v / 2.0]);
        assert_closed_halves(&strips, &plane([1.0, 0.0, 0.0], 8.0), [v * 0.4, v * 0.6]);
    }

    #[test]
    fn groove_floor_in_the_plane_is_not_doubled_by_the_cap() {
        // A 20 × 10 × 20 block with a V-groove from the top down to z = 10, along y. Cut at
        // z = 10, the groove's floor edge runs right across the outline, and both groove walls
        // touch the plane along it from above.
        let outline = [
            [0.0, 0.0],
            [20.0, 0.0],
            [20.0, 20.0],
            [12.0, 20.0],
            [10.0, 10.0],
            [8.0, 20.0],
            [0.0, 20.0],
        ];
        let mut w = Welder::new(0);
        let at = |p: [f32; 2], y: f32| [p[0], y, p[1]];
        // The outline runs counter-clockwise in (x, z), which faces -y.
        for i in 0..outline.len() {
            let (p, q) = (outline[i], outline[(i + 1) % outline.len()]);
            w.triangle(at(p, 0.0), at(q, 10.0), at(q, 0.0));
            w.triangle(at(p, 0.0), at(p, 10.0), at(q, 10.0));
        }
        // End caps: a fan from the groove's tip, which sees every corner.
        for i in 5..10 {
            let (p, q) = (outline[i % 7], outline[(i + 1) % 7]);
            w.triangle(at(outline[4], 0.0), at(p, 0.0), at(q, 0.0));
            w.triangle(at(outline[4], 10.0), at(q, 10.0), at(p, 10.0));
        }
        let block = w.finish().unwrap();
        let s = stats(&block);
        assert_eq!((s.open_edges, s.non_manifold_edges), (0, 0));
        assert!((s.volume - 3800.0).abs() < 1e-9, "{}", s.volume);
        assert_closed_halves(&block, &plane(Z, 10.0), [2000.0, 1800.0]);
    }

    #[test]
    fn flat_faced_parts_stay_closed_on_vertex_rows_and_faces() {
        // CAD-like parts cut where it is hardest: on their vertex rows and faces (every quarter
        // cell along each axis), and along a few tilted directions through their corners.
        let shapes = [
            voxels([3, 3, 2], 10.0, |x, y, z| (x, y) != (1, 1) || z == 0),
            voxels([5, 5, 5], 1.0, |x, y, z| {
                [x, y, z].iter().any(|&c| c == 0 || c == 4) || (x, y, z) == (2, 2, 2)
            }),
            voxels([3, 2, 3], 2.0, |x, _, z| !(x == 1 && z > 0) || z == 2),
        ];
        let tilted = [
            [1.0, 1.0, 0.0],
            [1.0, -1.0, 1.0],
            [2.0, 1.0, 3.0],
            [0.0, 1.0, 1.0],
        ];
        for (n, shape) in shapes.iter().enumerate() {
            let s = stats(shape);
            assert_eq!((s.open_edges, s.non_manifold_edges), (0, 0), "shape {n}");
            let [lo, hi] = crate::mesh::bounds(shape).unwrap();
            let mut planes = Vec::new();
            for k in 0..3 {
                let mut normal = [0.0; 3];
                normal[k] = 1.0;
                let steps = ((hi[k] - lo[k]) * 4.0).round() as i32;
                for i in 0..=steps {
                    let at = lo[k] as f64 + (hi[k] - lo[k]) as f64 * i as f64 / steps as f64;
                    planes.push(plane(normal, at));
                }
            }
            for t in tilted {
                let normal = plane(t, 0.0).normal;
                for v in 0..shape.vertex_count() as u32 {
                    planes.push(Plane {
                        normal,
                        offset: dot(normal, shape.point(v)),
                    });
                }
            }
            for p in &planes {
                let pieces = cut(shape, p, true);
                assert!(!pieces.open_outline, "shape {n} {p:?}");
                let mut sum = 0.0;
                for m in [&pieces.below, &pieces.above].into_iter().flatten() {
                    let s = stats(m);
                    assert_eq!(
                        (s.open_edges, s.non_manifold_edges),
                        (0, 0),
                        "shape {n} {p:?}"
                    );
                    sum += s.volume;
                }
                assert!((sum - s.volume).abs() < 1e-7 * s.volume, "shape {n} {p:?}");
            }
        }
    }

    #[test]
    fn loops_split_at_touching_points_and_peel_loose_chains() {
        // Two triangles touching at vertex 0, plus a loose chain 0-7-8.
        let edges = [
            (0, 1),
            (1, 2),
            (2, 0),
            (0, 3),
            (3, 4),
            (4, 0),
            (0, 7),
            (7, 8),
        ];
        let points = [
            [0.0, 0.0],
            [2.0, 1.0],
            [2.0, -1.0],
            [-2.0, -1.0],
            [-2.0, 1.0],
            [0.0, 0.0],
            [0.0, 0.0],
            [0.0, 3.0],
            [0.0, 5.0],
        ];
        let (found, open) = loops(&edges, &|v| points[v as usize]);
        assert!(open);
        assert!(found.iter().all(|l| l.along == Some(true)));
        let mut found: Vec<Vec<u32>> = found.into_iter().map(|l| l.ids).collect();
        for l in &mut found {
            let k = l.iter().position(|&v| v == 0).unwrap();
            l.rotate_left(k);
        }
        found.sort();
        assert_eq!(found, vec![vec![0, 1, 2], vec![0, 3, 4]]);
    }

    #[test]
    fn t_junction_is_fanned() {
        // A square ring with a midpoint on its bottom side, capped by two triangles that skip it.
        let pts = [[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0], [1.0, 0.0]];
        let flat = |v: u32| pts[v as usize];
        let ring = Loop {
            ids: vec![0, 4, 1, 2, 3],
            along: Some(true),
            touches: false,
        };
        let rings = vec![Ring::new(ring, &flat)];
        for near in [Some(1e-9), None] {
            let mut cap = vec![[0, 1, 2], [0, 2, 3]];
            let mut uses = edge_uses(&rings, &[0], &cap);
            mend_t_junctions(&mut cap, &mut uses, &rings, &[0], &flat, near);
            assert!(uses.values().all(|&n| n == 2), "{uses:?}");
            assert_eq!(cap, vec![[0, 4, 2], [0, 2, 3], [4, 1, 2]]);
            assert_eq!(flaws(&cap, &uses), 0);
        }
    }

    #[test]
    fn chain_steps_over_points_rounded_to_one_spot() {
        // Outline 0-2-3-4-1 along cap edge 0-1, with 2 and 3 rounded to the same point (as along
        // a fine cylinder 1 m from the origin): the chain runs through both, in outline order.
        let pts = [
            [0.0, 0.0],
            [4.0, 0.0],
            [1.0, 0.0],
            [1.0, 0.0],
            [2.0, 0.0],
            [2.0, 3.0],
        ];
        let flat = |v: u32| pts[v as usize];
        let uses: IdMap<Edge, u32> = [(0, 1), (0, 2), (2, 3), (3, 4), (1, 4)]
            .into_iter()
            .map(|e| (e, 1))
            .collect();
        let once_at = OnceEdges::new(&uses);
        let chain = Chain {
            ends: ((0, pts[0]), (1, pts[1])),
            apex: 5,
            once_at: &once_at,
            uses: &uses,
            flat: &flat,
        };
        let (mut out, mut scratch) = (Vec::new(), Vec::new());
        assert!(chain_along(&chain, 1e-9, &mut out, &mut scratch));
        assert_eq!(out, vec![(0.25, 2), (0.25, 3), (0.5, 4)]);
    }

    #[test]
    fn chain_never_runs_through_the_opposite_corner() {
        // Cap triangle 0-1-2 over a square ring whose bottom side 0-1 is the chain 0-3-1, with the
        // apex 2 put on that line too (as a flat triangle's corner can be): only 3 may be fanned
        // in, never the apex, which would give a triangle 2-2-x.
        let pts = [[0.0, 0.0], [2.0, 0.0], [0.5, 0.0], [1.0, 0.0]];
        let flat = |v: u32| pts[v as usize];
        let mut uses: IdMap<Edge, u32> = [((0, 3), 1), ((1, 3), 1), ((0, 2), 1), ((1, 2), 1)]
            .into_iter()
            .collect();
        uses.insert((0, 1), 1);
        let once_at = OnceEdges::new(&uses);
        let chain = Chain {
            ends: ((0, pts[0]), (1, pts[1])),
            apex: 2,
            once_at: &once_at,
            uses: &uses,
            flat: &flat,
        };
        let (mut out, mut scratch) = (Vec::new(), Vec::new());
        assert!(chain_along(&chain, 1e-9, &mut out, &mut scratch));
        assert_eq!(out, vec![(0.5, 3)]);
        chain_exact(&chain, &mut out);
        assert_eq!(out, vec![(0.5, 3)]);
    }

    fn turned_over(mut mesh: Mesh) -> Mesh {
        for t in mesh.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
        mesh
    }

    /// Edges two triangles run the same way: the piece is not consistently wound there.
    fn misoriented(mesh: &Mesh) -> usize {
        let mut runs: HashMap<Edge, u32> = HashMap::new();
        for t in mesh.indices.as_chunks::<3>().0 {
            for k in 0..3 {
                *runs.entry((t[k], t[(k + 1) % 3])).or_default() += 1;
            }
        }
        runs.values().filter(|&&n| n > 1).count()
    }

    /// The 4 × 4 × 4 voxel solid whose cell (x, y, z) is bit x * 16 + y * 4 + z of `bits`, as the
    /// reviewer's random solids are numbered.
    fn cells(bits: u64) -> Mesh {
        voxels([4, 4, 4], 1.0, |x, y, z| {
            bits >> (x * 16 + y * 4 + z) & 1 == 1
        })
    }

    /// Both pieces closed, manifold and consistently wound, and their volumes add up.
    fn assert_closed_cut(mesh: &Mesh, p: &Plane, what: &str) {
        let whole = stats(mesh).volume;
        let pieces = cut(mesh, p, true);
        assert!(!pieces.open_outline, "{what}");
        let mut sum = 0.0;
        for m in [&pieces.below, &pieces.above].into_iter().flatten() {
            let s = stats(m);
            assert_eq!(
                (s.open_edges, s.non_manifold_edges, misoriented(m)),
                (0, 0, 0),
                "{what}"
            );
            sum += s.volume;
        }
        assert!(
            (sum - whole).abs() < 1e-7 * whole.abs(),
            "{what}: {sum} vs {whole}"
        );
    }

    #[test]
    fn overlapping_and_nested_shells_get_a_cap_each() {
        // Merge parts keeps every shell as it was, so one part can hold shells that cross or nest,
        // and one defect anywhere sends it here instead of to manifold-3d.
        let a = make_box([0.0; 3], [20.0; 3]);
        let mut b = make_box([10.0, 10.0, 0.0], [30.0, 30.0, 20.0]);
        b.indices.drain(6..9); // one of b's top triangles
        let crossing = concat(&[&a, &b]).unwrap();
        let pieces = cut(&crossing, &plane(Z, 10.0), true);
        assert!(!pieces.open_outline);
        let [below, above] = [&pieces.below, &pieces.above].map(|m| m.as_ref().unwrap());
        let s = stats(below);
        assert_eq!((s.open_edges, s.non_manifold_edges), (0, 0));
        assert_eq!(misoriented(below), 0);
        assert!((s.volume - 8000.0).abs() < 1e-9, "{}", s.volume);
        // The upper piece keeps the missing triangle's edges open, and only those.
        let s = stats(above);
        assert_eq!((s.open_edges, s.non_manifold_edges), (3, 0));
        assert_eq!(misoriented(above), 0);
        // A shell inside another, both facing out: two caps, one over the other.
        let inner = make_box([5.0; 3], [15.0; 3]);
        let nested = concat(&[&a, &inner]).unwrap();
        assert_closed_halves(&nested, &plane(Z, 10.0), [4500.0, 4500.0]);
        assert_closed_cut(&nested, &plane(Z, 10.0), "nested");
        // Turned inside out, the inner shell is a cavity: one cap with a hole.
        let hollow = concat(&[&a, &turned_over(inner)]).unwrap();
        assert_closed_halves(&hollow, &plane(Z, 10.0), [3500.0, 3500.0]);
    }

    #[test]
    fn inside_out_parts_with_holes_still_cap() {
        let frame = turned_over(voxels([3, 3, 1], 10.0, |x, y, _| (x, y) != (1, 1)));
        assert_closed_halves(&frame, &plane(Z, 5.0), [-4000.0, -4000.0]);
        let nested = turned_over(voxels([5, 5, 5], 1.0, |x, y, z| {
            [x, y, z].iter().any(|&c| c == 0 || c == 4) || (x, y, z) == (2, 2, 2)
        }));
        let total = -(125.0 - 27.0 + 1.0);
        assert_closed_halves(&nested, &plane(Z, 2.5), [total / 2.0, total / 2.0]);
    }

    #[test]
    fn loops_wound_both_ways_nest_by_depth() {
        // A frame with the x = 0 face of its outer wall turned over: the outer loop runs one edge
        // against the others and only the hole's loop runs one way, so winding cannot say which
        // loop is the hole.
        let frame = voxels([3, 3, 1], 10.0, |x, y, _| (x, y) != (1, 1));
        let mut flipped = frame.clone();
        for t in flipped.indices.as_chunks_mut::<3>().0 {
            if t.iter().all(|&v| frame.positions[v as usize * 3] == 0.0) {
                t.swap(1, 2);
            }
        }
        assert!(misoriented(&flipped) > 0);
        let p = plane(Z, 5.0);
        let (normal, turned) = (cut(&frame, &p, true), cut(&flipped, &p, true));
        assert!(!turned.open_outline);
        for (m, n) in [
            (&turned.below, &normal.below),
            (&turned.above, &normal.above),
        ] {
            let (s, t) = (stats(m.as_ref().unwrap()), stats(n.as_ref().unwrap()));
            assert_eq!((s.open_edges, s.non_manifold_edges), (0, 0));
            // The same cap: an annulus, the hole not filled over.
            assert!((s.area - t.area).abs() < 1e-9, "{} vs {}", s.area, t.area);
        }
    }

    #[test]
    fn hole_in_line_with_an_outline_edge_is_mended() {
        // A 40 × 40 plate, 5 mm thick, with a notch at the top and a square hole whose left side
        // is in line with the notch's right side (x = 20): earcut links the hole along that line,
        // over outline edges, and one cap edge only partly covers the next.
        let rows = ["..#.", ".###", "##.#", "####"];
        let plate = voxels([4, 4, 1], 10.0, |x, y, _| {
            rows[3 - y as usize].as_bytes()[x as usize] == b'#'
        });
        let plate = Mesh {
            positions: plate
                .positions
                .iter()
                .enumerate()
                .map(|(i, &v)| if i % 3 == 2 { v / 2.0 } else { v })
                .collect(),
            ..plate
        };
        assert_eq!(stats(&plate).volume, 5500.0);
        for z in [2.5, 1.0, 4.0] {
            assert_closed_halves(&plate, &plane(Z, z), [1100.0 * z, 1100.0 * (5.0 - z)]);
        }
        // The reviewer's random solids where the same happened.
        for (bits, normal, offset) in [
            (0x0d5f_5f09_4f9b_ee3b, Z, 0.5),
            (0x0632_3393_0fbf_8ce4, Z, 1.5),
            (0x8bfe_2ffb_64ef_ecec, [0.0, 1.0, 0.0], 1.0),
        ] {
            assert_closed_cut(&cells(bits), &plane(normal, offset), &format!("{bits:#x}"));
        }
    }

    #[test]
    fn holes_touching_their_outline_are_filled() {
        // From the reviewer's sweep, failing before any of these fixes: a hole whose three corners
        // sit on the outline (the walk now goes round the three pieces between them), and a hole
        // touching the outline at one vertex (now spliced into the outline there).
        let third = 0.5773502691896258;
        let p = Plane {
            normal: [-third, third, third],
            offset: 1.7320508075688776,
        };
        assert_closed_cut(&cells(0x0632_3393_0fbf_8ce4), &p, "three corners");
        let p = Plane {
            normal: [0.8164965809277261, -0.4082482904638631, -0.4082482904638631],
            offset: 0.0,
        };
        assert_closed_cut(&cells(0x7339_f72b_ffee_07fb), &p, "one vertex");
    }

    #[test]
    fn flat_earcut_triangles_are_dropped() {
        // Tilted cuts through voxel vertices whose outlines have points exactly in a line: earcut
        // cut off a triangle with no area there, which doubled two edges in the first and stood in
        // for a missing triangle in the second.
        let p = Plane {
            normal: [-1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0],
            offset: 5.0 / 3.0,
        };
        assert_closed_cut(&cells(0x1173_70fb_fdff_7fb9), &p, "doubled edges");
        let p = Plane {
            normal: [0.8164965809277261, -0.4082482904638631, 0.4082482904638631],
            offset: 2.4494897427831783,
        };
        assert_closed_cut(&cells(0xcdfd_15f7_ddff_55c1), &p, "missing triangle");
    }

    #[test]
    fn part_edge_as_a_diagonal_moves_off_it() {
        // Cap triangles a-b-c and b-a-d share a-b, an edge of the part in the plane. With d at
        // (1.5, -1) the quad a-d-b-c is convex and a-b flips to c-d; with d at (3, -0.1) it is
        // not, and a new vertex in the middle of a-b splits both triangles.
        for (d, flipped) in [([1.5, -1.0], true), ([3.0, -0.1], false)] {
            let positions = [0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 1.0, 1.0, 0.0, d[0], d[1], 0.0];
            let flat = |v: u32| {
                [
                    positions[v as usize * 3] as f64,
                    positions[v as usize * 3 + 1] as f64,
                ]
            };
            let mut cap = vec![[0, 1, 2], [1, 0, 3]];
            let mut uses: IdMap<Edge, u32> = [((0, 1), 2), ((0, 2), 2), ((1, 2), 2)]
                .into_iter()
                .chain([((1, 3), 2), ((0, 3), 2)])
                .collect();
            let touching: IdSet<Edge> = [(0, 1)].into_iter().collect();
            let mut points = Vec::new();
            assert!(avoid_part_edges(
                &mut cap,
                &mut uses,
                &touching,
                &flat,
                &positions,
                &mut points,
            ));
            assert!(!uses.contains_key(&(0, 1)));
            assert!(uses.values().all(|&n| n == 2), "{uses:?}");
            if flipped {
                assert!(points.is_empty());
                assert_eq!(cap, vec![[0, 3, 2], [3, 1, 2]]);
            } else {
                assert_eq!(points, vec![[1.0, 0.0, 0.0]]);
                assert_eq!(cap, vec![[0, 4, 2], [1, 4, 3], [4, 1, 2], [4, 0, 3]]);
            }
        }
    }

    #[test]
    fn part_edge_across_a_concave_quad_is_split() {
        // From the reviewer's sweep: a tilted plane through a vertex of a voxel solid holds an edge
        // of the part that touches it from one side, and earcut used that edge as the diagonal
        // of a quad that is not convex, so it cannot be flipped away.
        let p = Plane {
            normal: [0.4472135954999579, 0.0, 0.8944271909999159],
            offset: 4.024922359499621,
        };
        assert_closed_cut(&cells(0xd5fe_f13a_c9bf_dd9d), &p, "mvox1");
    }

    #[test]
    fn simplify_drops_only_rounding_off_straight_runs() {
        let tol = 2.0 * f32::EPSILON as f64 * 50.0;
        let round = |p: [f64; 2]| p.map(|v| v as f32 as f64);
        // A 50 mm square turned 30 degrees, 100 points a side, each rounded to f32 off its line.
        let (s, c) = (0.5, 0.75f64.sqrt());
        let mut square = Vec::new();
        for side in 0..4 {
            for i in 0..100 {
                let t = i as f64 / 2.0;
                let p = [[t, 0.0], [50.0, t], [50.0 - t, 50.0], [0.0, 50.0 - t]][side];
                square.push(round([p[0] * c - p[1] * s, p[0] * s + p[1] * c]));
            }
        }
        assert_eq!(simplify(&square, tol), vec![300, 0, 100, 200]);
        // A circle keeps every point while its chords bow out by more than tol...
        let circle = |n: usize| -> Vec<[f64; 2]> {
            (0..n)
                .map(|i| {
                    let a = std::f64::consts::TAU * i as f64 / n as f64;
                    round([50.0 * a.cos(), 50.0 * a.sin()])
                })
                .collect()
        };
        assert_eq!(simplify(&circle(1000), tol).len(), 1000);
        // ...and once they do not, every point it drops is within tol of its segment.
        let fine = circle(100_000);
        let keep = simplify(&fine, tol);
        assert!(keep.len() < fine.len() / 2, "{}", keep.len());
        for (k, &a) in keep.iter().enumerate() {
            let b = keep[(k + 1) % keep.len()];
            let (pa, pb) = (fine[a], fine[b]);
            let d = [pb[0] - pa[0], pb[1] - pa[1]];
            let length = d[0].hypot(d[1]);
            let mut i = (a + 1) % fine.len();
            while i != b {
                let r = [fine[i][0] - pa[0], fine[i][1] - pa[1]];
                assert!(
                    (d[0] * r[1] - d[1] * r[0]).abs() / length <= tol,
                    "point {i}"
                );
                let t = (r[0] * d[0] + r[1] * d[1]) / (length * length);
                assert!(t > 0.0 && t < 1.0, "point {i}");
                i = (i + 1) % fine.len();
            }
        }
    }

    #[test]
    fn nesting_index_agrees_with_testing_every_ring() {
        // Circles of many points nested three deep around a lattice of squares, some of them with
        // a square inside, and a square far off.
        let mut points: Vec<[f64; 2]> = Vec::new();
        let mut loops: Vec<Vec<u32>> = Vec::new();
        let mut add = |ring: Vec<[f64; 2]>| {
            let first = points.len() as u32;
            loops.push((first..first + ring.len() as u32).collect());
            points.extend(ring);
        };
        for (radius, n) in [(100.0, 400), (90.0, 300), (80.0, 50)] {
            add((0..n)
                .map(|i| {
                    let a = std::f64::consts::TAU * i as f64 / n as f64;
                    [radius * a.cos(), radius * a.sin()]
                })
                .collect());
        }
        let square = |x: f64, y: f64, h: f64| vec![[x, y], [x + h, y], [x + h, y + h], [x, y + h]];
        for i in 0..10 {
            for j in 0..10 {
                let (x, y) = (i as f64 * 10.0 - 50.0, j as f64 * 10.0 - 50.0);
                add(square(x, y, 6.0));
                if (i + j) % 2 == 0 {
                    add(square(x + 1.0, y + 1.0, 4.0));
                }
            }
        }
        add(square(500.0, 500.0, 1.0));
        let flat = |v: u32| points[v as usize];
        let rings: Vec<Ring> = loops
            .into_iter()
            .map(|ids| {
                let touches = false;
                let along = Some(true);
                Ring::new(
                    Loop {
                        ids,
                        along,
                        touches,
                    },
                    &flat,
                )
            })
            .collect();
        let live: Vec<usize> = (0..rings.len()).collect();
        let index = Containers::new(&rings, &live);
        let mut rows: Vec<Option<Rows>> = (0..rings.len()).map(|_| None).collect();
        let got: Vec<Vec<usize>> = (0..rings.len())
            .map(|r| {
                let mut around = Vec::new();
                index.around(&rings, r, &mut rows, &mut |o| around.push(o));
                around.sort_unstable();
                around
            })
            .collect();
        for r in 0..rings.len() {
            let want: Vec<usize> = (0..rings.len())
                .filter(|&o| o != r && rings[o].contains(rings[r].probe(), None))
                .collect();
            assert_eq!(got[r], want, "ring {r}");
        }
        // Squares inside the inner circle are three deep, the ones inside them four.
        assert_eq!(got[3].len(), 3);
        assert_eq!(got[4].len(), 4);
        assert!(got.last().unwrap().is_empty());
    }

    #[test]
    fn many_holes_are_linked_through_the_grid() {
        // A plate with 20 × 20 square holes, more than MANY_HOLES.
        let plate = voxels([41, 41, 1], 1.0, |x, y, _| x % 2 == 0 || y % 2 == 0);
        let v = stats(&plate).volume;
        assert_eq!(v, 41.0 * 41.0 - 400.0);
        assert_closed_halves(&plate, &plane(Z, 0.5), [v / 2.0, v / 2.0]);
        // bridge_holes links them all itself (earcut would otherwise do it in the fallback).
        let mut coords = vec![0.0, 0.0, 41.0, 0.0, 41.0, 41.0, 0.0, 41.0];
        let mut starts = Vec::new();
        for i in 0..20 {
            for j in 0..20 {
                starts.push(coords.len() / 2);
                let (x, y) = ((2 * i + 1) as f64, (2 * j + 1) as f64);
                coords.extend([x, y, x, y + 1.0, x + 1.0, y + 1.0, x + 1.0, y]);
            }
        }
        let pieces = bridge_holes(&coords, &starts).unwrap();
        // A link to the left and one to the right per hole: each right one splits off a piece.
        assert_eq!(pieces.len(), 401);
        let nodes: usize = pieces.iter().map(Vec::len).sum();
        assert_eq!(nodes, 4 + 400 * 4 + 4 * 400);
        let mut triangles: Vec<usize> = Vec::new();
        for piece in &pieces {
            let along: Vec<f64> = piece
                .iter()
                .flat_map(|&i| [coords[2 * i], coords[2 * i + 1]])
                .collect();
            let cut = earcutr::earcut(&along, &[], 2).unwrap();
            triangles.extend(cut.into_iter().map(|k| piece[k]));
        }
        assert!(covers(&coords, &starts, &triangles));
        // A row of square holes along u: each hole's left link runs to the one before, and its
        // right link, which would repeat that, is made from its other corner. Each gap between
        // two holes is then a piece of its own, and the strips below and above the row are
        // straight runs between a few corners, which `simplify` leaves earcut. (The row once
        // stayed on two pieces with every hole on them: 12,000 holes took seconds and ran out the
        // mending.)
        let mut coords = vec![0.0, 0.0, 60.0, 0.0, 60.0, 3.0, 0.0, 3.0];
        let mut starts = Vec::new();
        for i in 0..20 {
            starts.push(coords.len() / 2);
            let x = (3 * i + 1) as f64;
            coords.extend([x, 1.0, x, 2.0, x + 1.0, 2.0, x + 1.0, 1.0]);
        }
        let row = bridge_holes(&coords, &starts).unwrap();
        assert_eq!(row.len(), 21);
        assert_eq!(row.iter().filter(|piece| piece.len() == 4).count(), 19);
        for strip in row.iter().filter(|piece| piece.len() > 4) {
            let points: Vec<[f64; 2]> = strip
                .iter()
                .map(|&i| [coords[2 * i], coords[2 * i + 1]])
                .collect();
            assert!(simplify(&points, 1e-9).len() <= 6, "{strip:?}");
        }
        // A hole poking out of the outer loop finds no link, and earcut gets the polygon back.
        let mut outside = coords.clone();
        outside.extend([-5.0, 20.0, -5.0, 21.0, -4.0, 21.0, -4.0, 20.0]);
        let mut starts = starts.clone();
        starts.push(coords.len() / 2);
        assert_eq!(bridge_holes(&outside, &starts), None);
    }

    /// `mesh` turned by the unit quaternion q (normalised here) and moved by `shift`, rounded to
    /// f32 once, and the rotation matrix.
    fn turned(mesh: &Mesh, q: [f64; 4], shift: [f64; 3]) -> (Mesh, [[f64; 3]; 3]) {
        let l = dot([q[1], q[2], q[3]], [q[1], q[2], q[3]]) + q[0] * q[0];
        let [w, x, y, z] = q.map(|v| v / l.sqrt());
        let r = [
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y - z * w),
                2.0 * (x * z + y * w),
            ],
            [
                2.0 * (x * y + z * w),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z - x * w),
            ],
            [
                2.0 * (x * z - y * w),
                2.0 * (y * z + x * w),
                1.0 - 2.0 * (x * x + y * y),
            ],
        ];
        let mut out = mesh.clone();
        for p in out.positions.as_chunks_mut::<3>().0 {
            let v = p.map(|c| c as f64);
            *p = [0, 1, 2].map(|k| (dot(r[k], v) + shift[k]) as f32);
        }
        (out, r)
    }

    #[test]
    fn grids_of_holes_turned_off_the_axes_stay_closed() {
        // The verifier's plates: 100 square holes in rows, turned and moved off the axes so every
        // crossing point is f32-rounded off its line, cut parallel to the faces and at shallow
        // tilts, whole and with one bottom triangle missing (the way such a part comes here).
        // Straightening those runs and dropping flat triangles once fanned two edges from one
        // corner through one point (an edge of four triangles) in 12 of these 72 cuts; such a
        // polygon is now filled the plain way.
        let plate = voxels([21, 21, 1], 1.0, |x, y, _| !(x % 2 == 1 && y % 2 == 1));
        let mut nicked = plate.clone();
        let bottom = nicked
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .position(|t| {
                t.iter()
                    .all(|&i| nicked.positions[i as usize * 3 + 2] == 0.0)
            })
            .unwrap();
        nicked.indices.drain(bottom * 3..bottom * 3 + 3);
        let mut seed = 0x5eedu64;
        let mut next = || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 11) as f64 / (1u64 << 52) as f64 - 1.0
        };
        for case in 0..12 {
            let q = [next(), next(), next(), next()];
            let shift = [next() * 50.0, next() * 50.0, next() * 50.0];
            let tilt = [next() * 0.05, next() * 0.05];
            for (part, defect) in [(&plate, 0), (&nicked, 3)] {
                let (mesh, r) = turned(part, q, shift);
                let turn = |v: [f64; 3]| [0, 1, 2].map(|k| dot(r[k], v));
                for (normal, at) in [
                    ([0.0, 0.0, 1.0], [0.0, 0.0, 0.5]),
                    ([0.0, 0.0, 1.0], [0.0, 0.0, 0.25]),
                    ([tilt[0], tilt[1], 1.0], [10.5, 10.5, 0.5]),
                ] {
                    let n = plane(turn(normal), 0.0).normal;
                    let p = Plane {
                        normal: n,
                        offset: dot(n, turn(at)) + dot(n, shift),
                    };
                    let what = format!("case {case}, defect {defect}, {p:?}");
                    let pieces = cut(&mesh, &p, true);
                    assert!(!pieces.open_outline, "{what}");
                    let mut open = 0;
                    for m in [&pieces.below, &pieces.above].into_iter().flatten() {
                        let s = stats(m);
                        assert_eq!((s.non_manifold_edges, misoriented(m)), (0, 0), "{what}");
                        open += s.open_edges;
                    }
                    assert_eq!(open, defect, "{what}");
                }
            }
        }
    }

    #[test]
    fn hole_crossing_another_shells_outline_goes_to_the_loop_around_all_of_it() {
        // A square tube (bore [3, 6]²) merged with a solid post [1, 5]² that overlaps a corner of
        // the bore: the bore's probe point lies inside the post's outline, but only the tube's
        // outer loop holds all of the bore. Nesting the bore in the post's loop gave earcut a hole
        // crossing its outline: 8 open edges and 409.5 mm³ below instead of 396.
        let tube = voxels([3, 3, 3], 3.0, |x, y, _| (x, y) != (1, 1));
        let post = make_box([1.0, 1.0, 0.0], [5.0, 5.0, 9.0]);
        let both = concat(&[&tube, &post]).unwrap();
        assert_closed_halves(&both, &plane(Z, 4.5), [396.0, 396.0]);
        let pieces = cut(&both, &plane(Z, 4.5), true);
        for m in [&pieces.below, &pieces.above] {
            assert_eq!(misoriented(m.as_ref().unwrap()), 0);
        }
        // A cavity flush with a solid's side (the cavity's wall at y = 25 in line with the solid's
        // y = 25 face) is not held by the solid's loop either, which would leave earcut a hole
        // along its outline.
        let hollow = make_box([-50.0, -50.0, 0.0], [50.0, 50.0, 20.0]);
        let cavity = turned_over(make_box([5.0, 5.0, 5.0], [25.0, 25.0, 15.0]));
        let solid = make_box([0.0, -5.0, 0.0], [30.0, 25.0, 20.0]);
        let all = concat(&[&hollow, &cavity, &solid]).unwrap();
        let whole = stats(&all).volume;
        assert_closed_halves(&all, &plane(Z, 10.0), [whole / 2.0, whole / 2.0]);
    }

    #[test]
    fn hole_under_a_slot_of_another_shell_is_not_that_shells_hole() {
        // The cavity [5, 25]² lies wholly inside a U-shaped shell's outline, but the U's slot
        // (x 12..18, y 20..30) runs into it: the U's slot corners lie inside the cavity, so the U
        // does not hold it, and it goes to the hollow box around both. As the U's hole it gave
        // 5 open edges and 80,750 mm³ below z = 7.5 instead of 80,300.
        let hollow = make_box([-50.0, -50.0, 0.0], [50.0, 50.0, 20.0]);
        let cavity = turned_over(make_box([5.0, 5.0, 5.0], [25.0, 25.0, 15.0]));
        let u = voxels([15, 15, 10], 2.0, |x, y, _| {
            !((6..9).contains(&x) && y >= 10)
        });
        let all = concat(&[&hollow, &cavity, &u]).unwrap();
        assert_closed_halves(&all, &plane(Z, 7.5), [80_300.0, 132_500.0]);
        assert_closed_halves(&all, &plane(Z, 12.5), [132_500.0, 80_300.0]);
    }
}
