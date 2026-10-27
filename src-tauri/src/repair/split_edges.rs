//! Split non-oriented edges and Split non-manifold edges: give the triangles on such edges their own
//! copies of the edge's vertices, so every edge ends up with at most two consistently oriented
//! triangles. Ignore the selection.
//!
//! Both first decide which sides of each edge stay glued together, then give every vertex one copy
//! per fan: the corners round the vertex that are joined through glued sides. Triangles keep their
//! order and every corner keeps its position; only the vertex a corner uses changes, the new
//! vertices being copies appended after the existing ones.

use std::cmp::Ordering;

use super::exact::{P3, collinear, dominant_axis, orient2d, orient3d, project, vertex};
use super::tiny_shells::plural;
use super::topology::{Edges, NONE, is_consistent, side_ends};
use super::{Concern, Control, Origin, Outcome, RepairError, RepairResult};
use crate::mesh::Mesh;

/// Sides between cancel checks.
const CHECK_EVERY: usize = 1 << 16;

/// Cuts every edge whose two sides run the same way (its triangles face opposite ways across it),
/// then gives each end of a cut edge one copy per fan. Non-manifold edges stay whole, and so does
/// every vertex no cut edge touches.
///
/// Where the two triangles of a cut edge are joined anyway round both of its ends, through other
/// edges, the copies cannot separate them and the edge stays as it was, with a warning. On an
/// orientable surface that needs a non-manifold edge in the way; a cube with a flipped triangle,
/// say, comes apart cleanly (the triangle becomes a shell of its own).
pub fn split_non_oriented(
    mesh: &Mesh,
    selection: &[u32],
    control: &mut Control,
) -> RepairResult<Outcome> {
    let _ = selection;
    control.check()?;
    let edges = Edges::new(mesh);
    control.progress(0.3);
    control.check()?;
    let mut glue: Vec<u32> = (0..mesh.indices.len() as u32).collect();
    let mut cut = Vec::new();
    let mut ends = vec![false; mesh.vertex_count()];
    for e in 0..edges.len() as u32 {
        match *edges.sides(e) {
            [_] => {}
            [s, r] => {
                if is_consistent(mesh, &edges, e) {
                    glue[s as usize] = r;
                    glue[r as usize] = s;
                } else {
                    cut.push([s, r]);
                    for v in edges.ends(e) {
                        ends[v as usize] = true;
                    }
                }
            }
            // A non-manifold edge stays whole: all its sides glued to the first.
            ref sides => {
                for &s in &sides[1..] {
                    glue[s as usize] = sides[0];
                }
            }
        }
    }
    drop(edges);
    if cut.is_empty() {
        return Ok(Outcome::unchanged(mesh, "No non-oriented edges to split."));
    }
    control.progress(0.4);
    let (vertex_of, copies) = split_fans(mesh, &glue, Some(&ends), control)?;
    control.progress(0.9);
    let split = cut
        .iter()
        .filter(|&&[s, r]| edge_key(&vertex_of, s) != edge_key(&vertex_of, r))
        .count();
    let stuck = cut.len() - split;
    let mut warnings = Vec::new();
    if stuck > 0 {
        warnings.push(format!(
            "{} could not be split, because {} triangles are joined through other edges round both ends.",
            plural(stuck, "non-oriented edge", "non-oriented edges"),
            if stuck == 1 { "its" } else { "their" }
        ));
    }
    let concerns: Vec<(usize, Concern)> = (0..warnings.len())
        .map(|i| (i, Concern::Misoriented))
        .collect();
    if split == 0 {
        let mut out = Outcome::unchanged(mesh, "No non-oriented edges could be split.");
        out.warnings = warnings;
        out.concerns = concerns;
        return Ok(out);
    }
    let out = with_copies(mesh, vertex_of, &copies)?;
    control.progress(1.0);
    Ok(Outcome {
        mesh: out,
        changed: true,
        origin: Origin::Same,
        message: format!(
            "Split {}.",
            plural(split, "non-oriented edge", "non-oriented edges")
        ),
        warnings,
        highlight: None,
        select: None,
        concerns: Vec::new(),
    })
}

/// Pairs the sides round every non-manifold edge so each pair encloses solid (pair_sides), then
/// gives every vertex one copy per fan. Afterwards no edge has more than two sides, and solids
/// that only touched along an edge or at a corner are separate solids.
///
/// Where surfaces cross along a non-manifold edge, or a fin or wall sits inside a solid, pairing
/// by angle joins the fans round both ends of the edge, which would leave more than two sides
/// between the same two copies. Such crowded edges are paired again within each solid: the
/// triangles joined through every glued side except those of crowded edges, which holds each
/// closed surface together however it crosses the others. Where that still crowds, the surface
/// round each end of the edge is one fan passing the edge twice (two wedges of one solid held
/// between a floor and a ceiling, say): pairing each closing side with the next opening side
/// instead, round the empty wedges, gives each end two fans. Whatever crowds after that is
/// unglued (unglue_crowded), which leaves no edge with more than two sides.
pub fn split_non_manifold(
    mesh: &Mesh,
    selection: &[u32],
    control: &mut Control,
) -> RepairResult<Outcome> {
    let _ = selection;
    control.check()?;
    let edges = Edges::new(mesh);
    control.progress(0.3);
    control.check()?;
    let mut glue: Vec<u32> = (0..mesh.indices.len() as u32).collect();
    let mut many = Vec::new();
    for e in 0..edges.len() as u32 {
        match *edges.sides(e) {
            [_] => {}
            [s, r] => {
                glue[s as usize] = r;
                glue[r as usize] = s;
            }
            _ => many.push(e),
        }
    }
    let mut fins = Vec::new();
    for (n, &e) in many.iter().enumerate() {
        if n % 4096 == 0 {
            control.check()?;
        }
        pair_sides(mesh, &edges, e, None, false, &mut glue, &mut fins);
    }
    control.progress(0.4);
    let (mut vertex_of, mut copies) = split_fans(mesh, &glue, None, control)?;
    let mut crowded = crowded_edges(&edges, &many, &vertex_of);
    if !crowded.is_empty() {
        control.check()?;
        let solid = solids(mesh, &edges, &glue, &crowded);
        for (round, void) in [false, true].into_iter().enumerate() {
            for &e in &crowded {
                for &s in edges.sides(e) {
                    glue[s as usize] = s;
                }
            }
            for &e in &crowded {
                pair_sides(mesh, &edges, e, Some(&solid), void, &mut glue, &mut fins);
            }
            control.progress(0.5 + 0.1 * round as f64);
            (vertex_of, copies) = split_fans(mesh, &glue, None, control)?;
            // New pairs can join fans anywhere round their vertices, so every edge is checked.
            crowded = crowded_edges(&edges, &many, &vertex_of);
            if crowded.is_empty() {
                break;
            }
        }
        if unglue_crowded(&edges, &crowded, &vertex_of, &mut glue) {
            control.progress(0.7);
            (vertex_of, copies) = split_fans(mesh, &glue, None, control)?;
        }
    }
    control.progress(0.9);
    if copies.is_empty() {
        return Ok(Outcome::unchanged(mesh, "No non-manifold edges to split."));
    }
    // Vertices that split although no non-manifold edge ends there: corners where separate fans
    // only touched.
    let mut counted = vec![false; mesh.vertex_count()];
    for &e in &many {
        for v in edges.ends(e) {
            counted[v as usize] = true;
        }
    }
    let mut pinched = 0;
    for &v in &copies {
        if !counted[v as usize] {
            counted[v as usize] = true;
            pinched += 1;
        }
    }
    let edges_text = plural(many.len(), "non-manifold edge", "non-manifold edges");
    let vertices_text = plural(pinched, "non-manifold vertex", "non-manifold vertices");
    let message = match (many.len(), pinched) {
        (_, 0) => format!("Split {edges_text}."),
        (0, _) => format!("Split {vertices_text}."),
        _ => format!("Split {edges_text} and {vertices_text}."),
    };
    let out = with_copies(mesh, vertex_of, &copies)?;
    control.progress(1.0);
    Ok(Outcome {
        mesh: out,
        changed: true,
        origin: Origin::Same,
        message,
        warnings: Vec::new(),
        highlight: None,
        select: None,
        concerns: Vec::new(),
    })
}

/// One side of a non-manifold edge, as pair_sides sorts them round the edge.
struct Fin {
    side: u32,
    /// Only sides of the same solid pair (all 0 for the first pairing).
    solid: u32,
    /// The side runs from the edge's higher vertex to its lower one. Seen from the higher vertex,
    /// angles round the edge grow counter-clockwise, and such a side's triangle has its back (the
    /// solid behind it) on the side of larger angles: it opens a wedge of solid. A side running
    /// the other way closes one.
    opens: bool,
    /// Where it lies round the edge: -1 for a triangle without area (its third corner on the
    /// edge's line), 0 for an angle in [0°, 180°) from the first triangle with area, 1 for
    /// [180°, 360°).
    half: i8,
    /// The triangle's third corner.
    apex: P3,
}

/// Pairs the sides round the non-manifold edge `e` so that each pair encloses solid, gluing each
/// side that opens a wedge to the next side round the edge that closes one, like brackets round a
/// circle: inner pairs first, so a solid wedge holding a fin or a smaller wedge pairs round it.
/// What is left runs one way only and stays unglued. With `solid` (per triangle), only sides of
/// the same solid pair, each solid's sides taken in their order round the edge. With `void`, each
/// closing side pairs with the next opening side instead, so the pairs enclose the empty wedges.
///
/// Angles are decided exactly (orient3d of the third corners against the edge). At equal angles
/// closing sides come first, so two solids touching face to face stay apart; among sides that
/// agree in everything the lowest triangle gets the pair, so a doubled triangle's first copy stays
/// in its surface on all three edges.
fn pair_sides(
    mesh: &Mesh,
    edges: &Edges,
    e: u32,
    solid: Option<&[u32]>,
    void: bool,
    glue: &mut [u32],
    fins: &mut Vec<Fin>,
) {
    let [lo, hi] = edges.ends(e);
    let (u, v) = (vertex(mesh, lo), vertex(mesh, hi));
    fins.clear();
    for &side in edges.sides(e) {
        let apex = vertex(
            mesh,
            mesh.indices[(side / 3 * 3 + (side % 3 + 2) % 3) as usize],
        );
        fins.push(Fin {
            side,
            solid: solid.map_or(0, |solid| solid[(side / 3) as usize]),
            opens: side_ends(mesh, side).0 == hi,
            half: if collinear(u, v, apex) { -1 } else { 0 },
            apex,
        });
    }
    if let Some(reference) = fins.iter().find(|f| f.half == 0).map(|f| f.apex)
        && let Some(axis) = dominant_axis(u, v, reference)
    {
        // A third corner in the reference's plane lies on the reference's half of it (0°) or the
        // other half (180°); the projection along the plane's dominant axis keeps that apart.
        let (pu, pv) = (project(u, axis), project(v, axis));
        let reference_side = orient2d(pu, pv, project(reference, axis));
        for f in fins.iter_mut().filter(|f| f.half == 0) {
            f.half = match orient3d(u, v, reference, f.apex) {
                1 => 0,
                -1 => 1,
                _ if orient2d(pu, pv, project(f.apex, axis)) == reference_side => 0,
                _ => 1,
            };
        }
    }
    fins.sort_by(|a, b| {
        a.solid
            .cmp(&b.solid)
            .then_with(|| round_the_edge(u, v, a, b))
    });
    for run in fins.chunk_by(|a, b| a.solid == b.solid) {
        pair_round(run, void, glue);
    }
}

/// Glues each opening side to the next closing side round the circle of `fins` (sorted round the
/// edge), cancelling inner pairs first like brackets; with `void`, each closing side to the next
/// opening side.
fn pair_round(fins: &[Fin], void: bool, glue: &mut [u32]) {
    let unglued = |glue: &[u32], f: &Fin| glue[f.side as usize] == f.side;
    let mut waiting: Vec<u32> = Vec::new();
    for f in fins {
        if f.opens != void {
            waiting.push(f.side);
        } else if let Some(s) = waiting.pop() {
            glue[s as usize] = f.side;
            glue[f.side as usize] = s;
        }
    }
    // Round the circle once more: sides that close before the first one that opens found
    // nothing to pair with; the sides still waiting at the end come right before them.
    for f in fins {
        let opens = f.opens != void;
        if waiting.is_empty() || (opens && unglued(glue, f)) {
            break;
        }
        if !opens
            && unglued(glue, f)
            && let Some(s) = waiting.pop()
        {
            glue[s as usize] = f.side;
            glue[f.side as usize] = s;
        }
    }
}

/// The order round the edge from `u` to `v`: by angle, then closing sides before opening ones,
/// then closing sides by ascending and opening sides by descending side number (the stack in
/// pair_round pairs the first closing side with the last opening one). Exact, and a total order.
fn round_the_edge(u: P3, v: P3, a: &Fin, b: &Fin) -> Ordering {
    a.half
        .cmp(&b.half)
        .then_with(|| {
            if a.half < 0 {
                Ordering::Equal
            } else {
                // Positive when b lies counter-clockwise of a, less than half a turn on.
                0.cmp(&orient3d(u, v, a.apex, b.apex))
            }
        })
        .then(a.opens.cmp(&b.opens))
        .then_with(|| {
            if a.opens {
                b.side.cmp(&a.side)
            } else {
                a.side.cmp(&b.side)
            }
        })
}

/// The corner after `c` in its triangle: where side `c` ends.
#[inline]
fn next_corner(c: u32) -> u32 {
    c / 3 * 3 + (c % 3 + 1) % 3
}

/// The corner of `side`'s triangle at `x`, one of the side's two ends.
#[inline]
fn corner_at(mesh: &Mesh, side: u32, x: u32) -> u32 {
    if mesh.indices[side as usize] == x {
        side
    } else {
        next_corner(side)
    }
}

/// The two vertices a side joins once corners have moved to `vertex_of`, as one sortable key.
fn edge_key(vertex_of: &[u32], side: u32) -> u64 {
    let (a, b) = (
        vertex_of[side as usize],
        vertex_of[next_corner(side) as usize],
    );
    (a.min(b) as u64) << 32 | a.max(b) as u64
}

/// The vertex each corner uses once every vertex has one copy per fan, and the vertex each new
/// vertex copies, in order.
///
/// A fan is a set of corners round one vertex joined through glued sides (two glued sides join
/// their triangles' corners at either end of the edge) and through zero-length sides (which join
/// the two corners of their triangle on the same vertex, so a triangle that repeats a vertex never
/// gets two of its copies). Union-find over corners, each set's root its lowest corner. The fan
/// holding a vertex's lowest corner keeps the vertex; further fans get new vertices, numbered
/// after the existing ones in order of their lowest corners. With `only`, vertices where it is
/// false keep one vertex for all their fans.
fn split_fans(
    mesh: &Mesh,
    glue: &[u32],
    only: Option<&[bool]>,
    control: &Control,
) -> RepairResult<(Vec<u32>, Vec<u32>)> {
    let corners = mesh.indices.len();
    let mut parent: Vec<u32> = (0..corners as u32).collect();
    for s in 0..corners as u32 {
        if (s as usize).is_multiple_of(CHECK_EVERY) {
            control.check()?;
        }
        let (a, b) = side_ends(mesh, s);
        if a == b {
            join(&mut parent, s, next_corner(s));
            continue;
        }
        let g = glue[s as usize];
        if g != s {
            join(&mut parent, s, corner_at(mesh, g, a));
            join(&mut parent, next_corner(s), corner_at(mesh, g, b));
        }
    }
    control.check()?;
    let base = mesh.vertex_count();
    let mut vertex_of = vec![0u32; corners];
    let mut taken = vec![false; base];
    let mut copies = Vec::new();
    for c in 0..corners {
        let v = mesh.indices[c];
        let r = root(&mut parent, c as u32) as usize;
        vertex_of[c] = if r < c {
            vertex_of[r]
        } else if !taken[v as usize] {
            taken[v as usize] = true;
            v
        } else if only.is_none_or(|only| only[v as usize]) {
            copies.push(v);
            (base + copies.len() - 1) as u32
        } else {
            v
        };
    }
    Ok((vertex_of, copies))
}

fn root(parent: &mut [u32], mut c: u32) -> u32 {
    while parent[c as usize] != c {
        let up = parent[parent[c as usize] as usize];
        parent[c as usize] = up;
        c = up;
    }
    c
}

/// Joins the sets of `a` and `b`; the lower root stays the root, so a root is its set's lowest
/// member.
fn join(parent: &mut [u32], a: u32, b: u32) {
    let (ra, rb) = (root(parent, a), root(parent, b));
    if ra != rb {
        parent[ra.max(rb) as usize] = ra.min(rb);
    }
}

/// The sides of edge `e` sorted into `keys` by the two vertex copies they join once corners have
/// moved to `vertex_of`: a run of more than two sides with the same key is a crowd, sides that
/// would still make a non-manifold edge.
fn group_sides(edges: &Edges, e: u32, vertex_of: &[u32], keys: &mut Vec<(u64, u32)>) {
    keys.clear();
    keys.extend(edges.sides(e).iter().map(|&s| (edge_key(vertex_of, s), s)));
    keys.sort_unstable();
}

/// The edges of `many` that would still be non-manifold with corners moved to `vertex_of`.
fn crowded_edges(edges: &Edges, many: &[u32], vertex_of: &[u32]) -> Vec<u32> {
    let mut keys = Vec::new();
    many.iter()
        .copied()
        .filter(|&e| {
            group_sides(edges, e, vertex_of, &mut keys);
            keys.chunk_by(|a, b| a.0 == b.0).any(|run| run.len() > 2)
        })
        .collect()
}

/// The solid of each triangle, as a union-find root: the triangles joined through glued sides,
/// except the sides of the `crowded` edges, whose pairs are in doubt.
fn solids(mesh: &Mesh, edges: &Edges, glue: &[u32], crowded: &[u32]) -> Vec<u32> {
    let mut doubtful = vec![false; edges.len()];
    for &e in crowded {
        doubtful[e as usize] = true;
    }
    let mut parent: Vec<u32> = (0..mesh.triangle_count() as u32).collect();
    for s in 0..mesh.indices.len() as u32 {
        let g = glue[s as usize];
        // Only sides on an edge are ever glued.
        if g != s && !doubtful[edges.edge_of(s) as usize] {
            join(&mut parent, s / 3, g / 3);
        }
    }
    (0..parent.len() as u32)
        .map(|t| root(&mut parent, t))
        .collect()
}

/// Unglues every pair of sides of an edge of `many` that would share its two vertex copies with a
/// further side, and says whether there were any.
///
/// Ungluing only splits fans, so sides that land apart stay apart and nothing new crowds: one more
/// split_fans leaves at most two sides on every edge. A fan's corners form a chain round their
/// vertex (each corner has two sides there, each glued to at most one other corner), so it has at
/// most two unglued sides, and two unglued sides can only share an edge as the two ends of the
/// chains round both of its vertices.
fn unglue_crowded(edges: &Edges, many: &[u32], vertex_of: &[u32], glue: &mut [u32]) -> bool {
    let mut keys: Vec<(u64, u32)> = Vec::new();
    let mut any = false;
    for &e in many {
        group_sides(edges, e, vertex_of, &mut keys);
        for run in keys.chunk_by(|a, b| a.0 == b.0) {
            if run.len() <= 2 {
                continue;
            }
            for &(_, s) in run {
                let g = glue[s as usize];
                if g != s {
                    glue[g as usize] = g;
                    glue[s as usize] = s;
                    any = true;
                }
            }
        }
    }
    any
}

/// The mesh with every corner moved to its vertex in `vertex_of` and the copies appended.
fn with_copies(mesh: &Mesh, vertex_of: Vec<u32>, copies: &[u32]) -> RepairResult<Mesh> {
    if mesh.vertex_count() + copies.len() >= NONE as usize {
        return Err(RepairError::Failed(
            "Splitting the edges would give the part more vertices than tricky3D can handle."
                .into(),
        ));
    }
    let mut positions = Vec::with_capacity(mesh.positions.len() + copies.len() * 3);
    positions.extend_from_slice(&mesh.positions);
    for &v in copies {
        positions.extend_from_slice(&mesh.vertex(v));
    }
    Ok(Mesh {
        positions,
        indices: vertex_of,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    use super::*;
    use crate::mesh::{make_box, stats, weld_indexed};
    use crate::ops::concat;
    use crate::repair::exact::testing::{Rng, torus};
    use crate::repair::shells::{ShellRow, shell_table};
    use crate::repair::status::{RepairStatus, analyze};
    use crate::repair::topology::{Link, components};
    use crate::repair::{RepairError, silent};

    fn cube() -> Mesh {
        make_box([0.0; 3], [20.0; 3])
    }

    fn flip(mesh: &mut Mesh, t: usize) {
        mesh.indices.swap(t * 3 + 1, t * 3 + 2);
    }

    /// Equal positions become one vertex, as an imported file is welded.
    fn weld(mesh: &Mesh) -> Mesh {
        weld_indexed(&mesh.positions, &mesh.indices).unwrap()
    }

    /// Meshes side by side in one part, welded where they share positions.
    fn glued(parts: &[&Mesh]) -> Mesh {
        weld(&concat(parts).unwrap())
    }

    /// A hexahedron over eight corners in make_box's order (bottom face counter-clockwise seen
    /// from above, then the top face), with make_box's triangles.
    fn hexahedron(corners: [[f32; 3]; 8]) -> Mesh {
        let mut m = make_box([0.0; 3], [1.0; 3]);
        m.positions = corners.concat();
        m
    }

    fn status(mesh: &Mesh) -> RepairStatus {
        silent(|c| analyze(mesh, c)).unwrap().status
    }

    fn non_manifold(mesh: &Mesh) -> Vec<u32> {
        let edges = Edges::new(mesh);
        (0..edges.len() as u32)
            .filter(|&e| edges.degree(e) > 2)
            .collect()
    }

    fn non_oriented(mesh: &Mesh) -> usize {
        let edges = Edges::new(mesh);
        (0..edges.len() as u32)
            .filter(|&e| edges.degree(e) == 2 && !is_consistent(mesh, &edges, e))
            .count()
    }

    /// The shells of a mesh sorted by triangle count then volume, for comparing.
    fn shells(mesh: &Mesh) -> Vec<ShellRow> {
        let mut rows = shell_table(mesh).0;
        rows.sort_by(|a, b| {
            a.triangles.cmp(&b.triangles).then(
                a.volume
                    .unwrap_or(-1.0)
                    .total_cmp(&b.volume.unwrap_or(-1.0)),
            )
        });
        rows
    }

    /// What every split must keep: the same triangles in the same order, every corner where it
    /// was, a second run that changes nothing.
    fn check_split(
        mesh: &Mesh,
        action: fn(&Mesh, &[u32], &mut Control) -> RepairResult<Outcome>,
    ) -> Outcome {
        let out = silent(|c| action(mesh, &[], c)).unwrap();
        assert_eq!(out.origin, Origin::Same);
        assert_eq!(out.mesh.triangle_count(), mesh.triangle_count());
        assert_eq!(
            out.mesh.positions[..mesh.positions.len()],
            mesh.positions[..]
        );
        for (&before, &after) in mesh.indices.iter().zip(&out.mesh.indices) {
            assert_eq!(mesh.vertex(before), out.mesh.vertex(after));
        }
        out.mesh.check().unwrap();
        if !out.changed {
            assert_eq!(out.mesh, *mesh);
        }
        let again = silent(|c| action(&out.mesh, &[], c)).unwrap();
        assert!(!again.changed, "second run: {}", again.message);
        out
    }

    fn split_nm(mesh: &Mesh) -> Outcome {
        let out = check_split(mesh, split_non_manifold);
        assert!(non_manifold(&out.mesh).is_empty(), "{}", out.message);
        out
    }

    fn split_no(mesh: &Mesh) -> Outcome {
        check_split(mesh, split_non_oriented)
    }

    fn closed_shell(triangles: u64, volume: f64) -> (u64, Option<f64>, bool) {
        (triangles, Some(volume), true)
    }

    fn summary(rows: &[ShellRow]) -> Vec<(u64, Option<f64>, bool)> {
        rows.iter()
            .map(|r| (r.triangles, r.volume, r.watertight))
            .collect()
    }

    #[test]
    fn a_clean_cube_is_unchanged() {
        for (out, message) in [
            (split_nm(&cube()), "No non-manifold edges to split."),
            (split_no(&cube()), "No non-oriented edges to split."),
        ] {
            assert!(!out.changed);
            assert_eq!(out.message, message);
            assert_eq!(out.mesh, cube());
        }
        assert!(!split_nm(&Mesh::default()).changed);
        assert!(!split_no(&Mesh::default()).changed);
    }

    #[test]
    fn two_cubes_touching_along_an_edge_come_apart() {
        let m = glued(&[&cube(), &make_box([20.0, 20.0, 0.0], [40.0, 40.0, 20.0])]);
        assert_eq!(non_manifold(&m).len(), 1);
        // Already two shells: a non-manifold edge joins none.
        assert_eq!(status(&m).shells, 2);
        let out = split_nm(&m);
        assert_eq!(out.message, "Split 1 non-manifold edge.");
        assert_eq!(out.mesh.vertex_count(), m.vertex_count() + 2);
        assert_eq!(
            summary(&shells(&out.mesh)),
            vec![closed_shell(12, 8000.0), closed_shell(12, 8000.0)]
        );
        let s = status(&out.mesh);
        assert!(s.closed && s.oriented, "{s:?}");
        assert_eq!(s.volume, 16000.0);
    }

    #[test]
    fn two_cubes_touching_at_a_corner_come_apart() {
        let m = glued(&[&cube(), &make_box([20.0; 3], [40.0; 3])]);
        assert!(non_manifold(&m).is_empty());
        // Already two shells: a shared corner joins none.
        assert_eq!(status(&m).shells, 2);
        let out = split_nm(&m);
        assert_eq!(out.message, "Split 1 non-manifold vertex.");
        assert_eq!(out.mesh.vertex_count(), m.vertex_count() + 1);
        assert_eq!(
            summary(&shells(&out.mesh)),
            vec![closed_shell(12, 8000.0), closed_shell(12, 8000.0)]
        );
        // Splitting non-oriented edges leaves the corner alone.
        assert!(!split_no(&m).changed);
    }

    #[test]
    fn edges_and_corners_are_reported_together() {
        let m = glued(&[
            &cube(),
            &make_box([20.0, 20.0, 0.0], [40.0, 40.0, 20.0]),
            &make_box([-20.0; 3], [0.0; 3]),
        ]);
        let out = split_nm(&m);
        assert_eq!(
            out.message,
            "Split 1 non-manifold edge and 1 non-manifold vertex."
        );
        assert_eq!(shells(&out.mesh).len(), 3);
        assert!(status(&out.mesh).closed);
    }

    #[test]
    fn cubes_touching_face_to_face_come_apart() {
        // Two cubes sharing a face, their diagonals crossing on it: its four sides are
        // non-manifold, the coplanar faces touch at equal angles.
        let two = glued(&[&cube(), &make_box([-20.0, 0.0, 0.0], [0.0, 20.0, 20.0])]);
        assert_eq!(non_manifold(&two).len(), 4);
        let out = split_nm(&two);
        assert_eq!(out.message, "Split 4 non-manifold edges.");
        assert_eq!(
            summary(&shells(&out.mesh)),
            vec![closed_shell(12, 8000.0); 2]
        );
        // A 2 × 2 × 2 block of cubes: every inner edge and corner is shared.
        let mut parts = Vec::new();
        for i in 0..8 {
            let o = [(i & 1) as f32, (i >> 1 & 1) as f32, (i >> 2) as f32].map(|k| k * 10.0);
            parts.push(make_box(o, o.map(|k| k + 10.0)));
        }
        let refs: Vec<&Mesh> = parts.iter().collect();
        let block = glued(&refs);
        let out = split_nm(&block);
        assert_eq!(
            summary(&shells(&out.mesh)),
            vec![closed_shell(12, 1000.0); 8]
        );
        let s = status(&out.mesh);
        assert!(s.closed && s.oriented && s.volume == 8000.0, "{s:?}");
    }

    #[test]
    fn three_and_four_cubes_round_one_edge_come_apart() {
        let a = cube();
        let b = make_box([-20.0, 0.0, 0.0], [0.0, 20.0, 20.0]);
        let c = make_box([-20.0, -20.0, 0.0], [0.0, 0.0, 20.0]);
        let d = make_box([0.0, -20.0, 0.0], [20.0, 0.0, 20.0]);
        for (parts, n) in [(vec![&a, &b, &c], 3), (vec![&a, &b, &c, &d], 4)] {
            let out = split_nm(&glued(&parts));
            assert_eq!(
                summary(&shells(&out.mesh)),
                vec![closed_shell(12, 8000.0); n]
            );
        }
        // Two cubes diagonally across the edge, nothing in the other two quarters.
        let out = split_nm(&glued(&[&a, &c]));
        assert_eq!(out.message, "Split 1 non-manifold edge.");
        assert_eq!(
            summary(&shells(&out.mesh)),
            vec![closed_shell(12, 8000.0); 2]
        );
    }

    #[test]
    fn boxes_crossing_along_a_shared_edge_stay_whole() {
        // A box turned 45° about the cube's edge at x = y = 0 shares that edge and cuts through
        // the cube: round the edge their faces alternate, and pairing by angle alone would join
        // the outer faces of both and the inner faces of both into one fan round each end.
        let b = |z: f32| {
            [
                [0.0, 0.0, z],
                [12.0, 12.0, z],
                [0.0, 24.0, z],
                [-12.0, 12.0, z],
            ]
        };
        let turned = hexahedron([b(0.0), b(20.0)].concat().try_into().unwrap());
        assert_eq!(stats(&turned).volume, 5760.0);
        let m = glued(&[&cube(), &turned]);
        assert_eq!(non_manifold(&m).len(), 1);
        let out = split_nm(&m);
        assert_eq!(out.message, "Split 1 non-manifold edge.");
        assert_eq!(
            summary(&shells(&out.mesh)),
            vec![closed_shell(12, 5760.0), closed_shell(12, 8000.0)]
        );
        assert!(status(&out.mesh).oriented);
        // With the cube's corner 0 also its first vertex and the other order of parts.
        let out = split_nm(&glued(&[&turned, &cube()]));
        assert_eq!(shells(&out.mesh).len(), 2);
        assert!(status(&out.mesh).closed);
    }

    #[test]
    fn fins_come_off_and_leave_the_solid_closed() {
        // A single triangle on the cube's edge x = y = 0, inside or outside, either winding.
        for apex in [
            [10.0, 10.0, 10.0],
            [-10.0, -10.0, 10.0],
            [10.0, -10.0, 10.0],
        ] {
            for turn in [false, true] {
                let mut fin = Mesh {
                    positions: [[0.0, 0.0, 0.0], [0.0, 0.0, 20.0], apex].concat(),
                    indices: vec![0, 1, 2],
                };
                if turn {
                    flip(&mut fin, 0);
                }
                let m = glued(&[&cube(), &fin]);
                assert_eq!(non_manifold(&m).len(), 1);
                let out = split_nm(&m);
                let rows = shells(&out.mesh);
                assert_eq!(rows.len(), 2, "{apex:?} {turn}");
                assert_eq!((rows[0].triangles, rows[0].watertight), (1, false));
                assert_eq!(summary(&rows[1..]), vec![closed_shell(12, 8000.0)]);
                assert!(status(&out.mesh).oriented);
            }
        }
        // Three fins on one edge (towards +y, +z and -y), all running 0 → 1: no two of them can
        // share the edge consistently, so all three come apart.
        let mut fins = Mesh {
            positions: vec![
                0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 0.0,
            ],
            indices: vec![0, 1, 2, 0, 1, 3, 0, 1, 4],
        };
        let out = split_nm(&fins);
        assert_eq!(out.message, "Split 1 non-manifold edge.");
        let s = status(&out.mesh);
        assert_eq!((s.border_edges, s.shells), (9, 3));
        // With the middle one turned, it pairs with the next one round the edge on its back
        // (counter-clockwise seen from vertex 1: +y, +z, -y), and the first stays apart.
        flip(&mut fins, 1);
        let out = split_nm(&fins);
        let s = status(&out.mesh);
        assert_eq!((s.border_edges, s.shells), (7, 2));
        let i = &out.mesh.indices;
        assert!(i[3..6].contains(&i[7]) && !i[3..6].contains(&i[0]));
    }

    /// A closed solid from convex polygons, each with its outward direction: each polygon is
    /// fanned from its first corner into triangles facing that way, and equal positions welded.
    fn polyhedron(faces: &[(Vec<[f32; 3]>, [f32; 3])]) -> Mesh {
        let mut soup = Mesh::default();
        for (corners, out) in faces {
            let p = |i: usize| corners[i].map(f64::from);
            let n = crate::repair::exact::normal(p(0), p(1), p(2));
            let mut order: Vec<usize> = (0..corners.len()).collect();
            if crate::repair::exact::dot(n, out.map(f64::from)) < 0.0 {
                order[1..].reverse();
            }
            let base = soup.vertex_count() as u32;
            for &i in &order {
                soup.positions.extend_from_slice(&corners[i]);
            }
            for i in 1..corners.len() as u32 - 1 {
                soup.indices
                    .extend_from_slice(&[base, base + i, base + i + 1]);
            }
        }
        weld(&soup)
    }

    #[test]
    fn two_wedges_between_a_floor_and_a_ceiling_get_two_fans_at_each_end() {
        // An H: a floor slab and a ceiling slab joined by two wedges that touch along the
        // vertical edge x = y = 0, z = 0..1. Round each end of that edge the surface is one fan
        // passing it twice (wedge, floor, other wedge, floor), so pairing the wedges' faces
        // leaves four sides on one edge; pairing round the empty wedges gives two fans per end.
        let ring = |z: f32, bottom: bool| -> Vec<(Vec<[f32; 3]>, [f32; 3])> {
            let up = if bottom { -1.0 } else { 1.0 };
            let face = |pts: &[[f32; 2]]| pts.iter().map(|&[x, y]| [x, y, z]).collect();
            vec![
                (
                    face(&[[0.0, 0.0], [3.0, 1.0], [4.0, 4.0], [-4.0, 4.0], [-3.0, 1.0]]),
                    [0.0, 0.0, up],
                ),
                (
                    face(&[
                        [0.0, 0.0],
                        [-3.0, -1.0],
                        [-4.0, -4.0],
                        [4.0, -4.0],
                        [3.0, -1.0],
                    ]),
                    [0.0, 0.0, up],
                ),
                (
                    face(&[[3.0, -1.0], [4.0, -4.0], [4.0, 4.0], [3.0, 1.0]]),
                    [0.0, 0.0, up],
                ),
                (
                    face(&[[-3.0, 1.0], [-4.0, 4.0], [-4.0, -4.0], [-3.0, -1.0]]),
                    [0.0, 0.0, up],
                ),
            ]
        };
        let slab = |z0: f32, z1: f32| -> Vec<(Vec<[f32; 3]>, [f32; 3])> {
            let c = [[-4.0, -4.0], [4.0, -4.0], [4.0, 4.0], [-4.0, 4.0]];
            let mut faces = Vec::new();
            for k in 0..4 {
                let ([x0, y0], [x1, y1]) = (c[k], c[(k + 1) % 4]);
                let out = [y1 - y0, x0 - x1, 0.0];
                faces.push((
                    vec![[x0, y0, z0], [x1, y1, z0], [x1, y1, z1], [x0, y0, z1]],
                    out,
                ));
            }
            faces
        };
        let wall = |[x0, y0]: [f32; 2], [x1, y1]: [f32; 2], out: [f32; 3]| {
            (
                vec![[x0, y0, 0.0], [x1, y1, 0.0], [x1, y1, 1.0], [x0, y0, 1.0]],
                out,
            )
        };
        let mut faces = Vec::new();
        faces.push((
            vec![
                [-4.0, -4.0, -1.0],
                [4.0, -4.0, -1.0],
                [4.0, 4.0, -1.0],
                [-4.0, 4.0, -1.0],
            ],
            [0.0, 0.0, -1.0],
        ));
        faces.push((
            vec![
                [-4.0, -4.0, 2.0],
                [4.0, -4.0, 2.0],
                [4.0, 4.0, 2.0],
                [-4.0, 4.0, 2.0],
            ],
            [0.0, 0.0, 1.0],
        ));
        faces.extend(slab(-1.0, 0.0));
        faces.extend(slab(1.0, 2.0));
        faces.extend(ring(0.0, false));
        faces.extend(ring(1.0, true));
        faces.push(wall([0.0, 0.0], [3.0, 1.0], [-1.0, 3.0, 0.0]));
        faces.push(wall([3.0, 1.0], [3.0, -1.0], [1.0, 0.0, 0.0]));
        faces.push(wall([3.0, -1.0], [0.0, 0.0], [-1.0, -3.0, 0.0]));
        faces.push(wall([0.0, 0.0], [-3.0, -1.0], [1.0, -3.0, 0.0]));
        faces.push(wall([-3.0, -1.0], [-3.0, 1.0], [-1.0, 0.0, 0.0]));
        faces.push(wall([-3.0, 1.0], [0.0, 0.0], [1.0, 3.0, 0.0]));
        let h = polyhedron(&faces);
        let before = status(&h);
        assert_eq!(
            (before.border_edges, before.non_manifold_edges),
            (0, 1),
            "{before:?}"
        );
        assert_eq!(before.volume, 134.0);
        // Pairing by angle crowds the edge; the H is one solid, so pairing within it does too.
        let (edges, glue, many) = paired_by_angle(&h);
        let (vertex_of, _) = silent(|c| split_fans(&h, &glue, None, c)).unwrap();
        assert_eq!(crowded_edges(&edges, &many, &vertex_of), many);
        assert!(solids(&h, &edges, &glue, &many).iter().all(|&s| s == 0));
        let out = split_nm(&h);
        assert_eq!(out.message, "Split 1 non-manifold edge.");
        assert_eq!(out.mesh.vertex_count(), h.vertex_count() + 2);
        let s = status(&out.mesh);
        assert!(s.closed && s.oriented, "{s:?}");
        assert_eq!((s.shells, s.volume), (1, 134.0));
    }

    #[test]
    fn a_double_sided_wall_inside_a_cube_comes_off() {
        // Two triangles back to back on the cube's edge x = y = 0, reaching into the cube.
        let wall = Mesh {
            positions: [[0.0, 0.0, 0.0], [0.0, 0.0, 20.0], [10.0, 10.0, 10.0]].concat(),
            indices: vec![0, 1, 2, 0, 2, 1],
        };
        let out = split_nm(&glued(&[&cube(), &wall]));
        let rows = shells(&out.mesh);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].triangles, rows[0].watertight), (2, true));
        assert_eq!(summary(&rows[1..]), vec![closed_shell(12, 8000.0)]);
    }

    #[test]
    fn a_doubled_triangle_comes_off_as_one_piece() {
        for reverse in [false, true] {
            for t in 0..12 {
                let mut m = cube();
                let mut copy = m.indices[t * 3..t * 3 + 3].to_vec();
                if reverse {
                    copy.swap(1, 2);
                }
                m.indices.extend(copy);
                assert_eq!(non_manifold(&m).len(), 3);
                let out = split_nm(&m);
                assert_eq!(out.message, "Split 3 non-manifold edges.");
                let rows = shells(&out.mesh);
                assert_eq!(rows.len(), 2, "triangle {t}, reversed {reverse}");
                assert_eq!((rows[0].triangles, rows[0].watertight), (1, false));
                assert_eq!(summary(&rows[1..]), vec![closed_shell(12, 8000.0)]);
                // The first copy stays in the cube.
                assert_eq!(out.mesh.indices[..36], cube().indices[..]);
            }
        }
    }

    #[test]
    fn a_tie_round_the_edge_needs_no_rounding() {
        // The face planes meet at exactly equal angles far from the origin, where an angle from
        // atan2 could round either way: closing sides still come first.
        let o = 1.0e6f32;
        let a = make_box([o; 3], [o + 0.5; 3]);
        let b = make_box([o - 0.5, o, o], [o, o + 0.5, o + 0.5]);
        let out = split_nm(&glued(&[&a, &b]));
        assert_eq!(out.message, "Split 4 non-manifold edges.");
        let rows = shells(&out.mesh);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.watertight && r.volume == Some(0.125)));
    }

    #[test]
    fn repeated_corners_stay_on_one_vertex() {
        // A triangle that repeats a vertex, sitting on a cube corner with a second cube touching
        // there: the corner splits, the degenerate triangle keeps one copy for both its corners.
        let mut m = glued(&[&cube(), &make_box([20.0; 3], [40.0; 3])]);
        let corner = m.indices[6 * 3 + 2];
        m.indices.extend_from_slice(&[corner, corner, 0]);
        let out = split_nm(&m);
        let t = &out.mesh.indices[out.mesh.indices.len() - 3..];
        assert_eq!(t[0], t[1]);
    }

    /// The mesh with its triangles in a random order.
    fn shuffled(mesh: &Mesh, rng: &mut Rng) -> Mesh {
        let mut order: Vec<usize> = (0..mesh.triangle_count()).collect();
        for i in (1..order.len()).rev() {
            order.swap(i, rng.below(i as u64 + 1) as usize);
        }
        Mesh {
            positions: mesh.positions.clone(),
            indices: order
                .iter()
                .flat_map(|&t| mesh.indices[t * 3..t * 3 + 3].to_vec())
                .collect(),
        }
    }

    /// 2 to 6 boxes with integer corners in 0..5, so they share corners, edges and faces, and
    /// overlap.
    fn grid_boxes(rng: &mut Rng) -> Vec<Mesh> {
        (0..2 + rng.below(5))
            .map(|_| {
                let o = [0, 1, 2].map(|_| rng.int(0, 3) as f32);
                make_box(o, [0, 1, 2].map(|k| o[k] + rng.int(1, 2) as f32))
            })
            .collect()
    }

    /// Boxes on a coarse grid, some inside out or with a triangle turned, loose fins and a doubled
    /// triangle, welded, often shuffled.
    fn tangle(rng: &mut Rng) -> Mesh {
        let mut parts = grid_boxes(rng);
        for part in &mut parts {
            if rng.chance(0.2) {
                for t in 0..12 {
                    flip(part, t);
                }
            }
            if rng.chance(0.2) {
                flip(part, rng.below(12) as usize);
            }
        }
        for _ in 0..rng.below(4) {
            parts.push(Mesh {
                positions: (0..9).map(|_| rng.int(0, 4) as f32).collect(),
                indices: vec![0, 1, 2],
            });
        }
        let refs: Vec<&Mesh> = parts.iter().collect();
        let mut m = glued(&refs);
        if rng.chance(0.3) {
            let t = rng.below(m.triangle_count() as u64) as usize;
            let copy = m.indices[t * 3..t * 3 + 3].to_vec();
            m.indices.extend(copy);
        }
        if rng.chance(0.5) {
            m = shuffled(&m, rng);
        }
        m
    }

    #[test]
    fn random_tangles_end_up_manifold() {
        // Afterwards no edge has more than two sides and every vertex has one fan (the second
        // run, which would split corners, changes nothing).
        let mut rng = Rng(77);
        for _ in 0..300 {
            let m = tangle(&mut rng);
            let out = split_nm(&m);
            split_no(&m);
            split_no(&out.mesh);
        }
    }

    /// The first step of split_non_manifold: twins glued, the non-manifold edges' sides paired by
    /// angle. Also the non-manifold edges.
    fn paired_by_angle(m: &Mesh) -> (Edges, Vec<u32>, Vec<u32>) {
        let edges = Edges::new(m);
        let mut glue: Vec<u32> = (0..m.indices.len() as u32).collect();
        let mut many = Vec::new();
        for e in 0..edges.len() as u32 {
            match *edges.sides(e) {
                [_] => {}
                [s, r] => {
                    glue[s as usize] = r;
                    glue[r as usize] = s;
                }
                _ => many.push(e),
            }
        }
        let mut fins = Vec::new();
        for &e in &many {
            pair_sides(m, &edges, e, None, false, &mut glue, &mut fins);
        }
        (edges, glue, many)
    }

    #[test]
    fn ungluing_alone_always_ends_the_crowding() {
        // The last resort on its own, straight after pairing by angle: once the crowded pairs are
        // unglued, one more split leaves no edge with more than two sides.
        let mut rng = Rng(79);
        let mut crowded = 0;
        for _ in 0..400 {
            let m = tangle(&mut rng);
            let (edges, mut glue, many) = paired_by_angle(&m);
            let (vertex_of, _) = silent(|c| split_fans(&m, &glue, None, c)).unwrap();
            let was = crowded_edges(&edges, &many, &vertex_of);
            assert_eq!(
                unglue_crowded(&edges, &many, &vertex_of, &mut glue),
                !was.is_empty()
            );
            crowded += usize::from(!was.is_empty());
            let (vertex_of, _) = silent(|c| split_fans(&m, &glue, None, c)).unwrap();
            assert!(crowded_edges(&edges, &many, &vertex_of).is_empty());
        }
        assert!(crowded > 20, "{crowded}");
    }

    #[test]
    fn outward_boxes_glued_on_a_grid_all_stay_closed() {
        // Boxes that touch along faces, edges and corners and overlap each other, all facing
        // outward: after the split every shell is closed and faces outward.
        let mut rng = Rng(78);
        for round in 0..300 {
            let parts = grid_boxes(&mut rng);
            let refs: Vec<&Mesh> = parts.iter().collect();
            let m = glued(&refs);
            let out = split_nm(&m);
            let rows = shell_table(&out.mesh).0;
            assert!(
                rows.iter().all(|r| r.watertight),
                "round {round}: {} -> {rows:?}",
                out.message
            );
            let s = status(&out.mesh);
            assert!(s.closed && s.oriented, "round {round}: {s:?}");
            // Each box is one shell or part of one with boxes it overlaps; nothing gets lost.
            let (_, touching) = components(&m, &Edges::new(&m), Link::Vertex);
            assert!(rows.len() >= touching);
        }
    }

    #[test]
    fn tori_stacked_on_a_shared_ring_come_apart() {
        // A torus with a diamond cross-section, and the same torus moved up so its bottom ring
        // lands on the first one's top ring: n non-manifold edges round a circle, each with two
        // wedges of solid touching at their tips.
        for n in [5usize, 8, 13] {
            let lower = torus(6.0, 2.0, n, 4);
            let mut upper = lower.clone();
            crate::ops::translate(&mut upper, [0.0, 0.0, 4.0]);
            let m = glued(&[&lower, &upper]);
            assert_eq!(non_manifold(&m).len(), n);
            let out = split_nm(&m);
            assert_eq!(out.message, format!("Split {n} non-manifold edges."));
            let s = status(&out.mesh);
            assert!(s.closed && s.oriented, "{s:?}");
            assert_eq!(s.shells, 2);
            assert!((s.volume - 2.0 * stats(&lower).volume).abs() < 1e-9 * s.volume);
        }
    }

    #[test]
    fn a_flipped_triangle_comes_off_along_its_non_oriented_edges() {
        let mut m = cube();
        flip(&mut m, 4);
        assert_eq!(non_oriented(&m), 3);
        let out = split_no(&m);
        assert_eq!(out.message, "Split 3 non-oriented edges.");
        assert!(out.warnings.is_empty());
        assert_eq!(non_oriented(&out.mesh), 0);
        let rows = shells(&out.mesh);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].triangles, 1);
        assert_eq!(rows[1].triangles, 11);
        let s = status(&out.mesh);
        assert_eq!((s.holes, s.border_edges), (2, 6));
        // Split non-manifold edges leaves non-oriented edges alone.
        assert!(!split_nm(&m).changed);
    }

    #[test]
    fn two_turned_faces_come_off_along_their_outline() {
        // The cube with its +z and +x faces turned: the six edges round the two faces are
        // non-oriented, the edge between them is not, so the two faces come off together.
        let mut m = cube();
        for t in [2, 3, 8, 9] {
            flip(&mut m, t);
        }
        assert_eq!(non_oriented(&m), 6);
        let out = split_no(&m);
        assert_eq!(out.message, "Split 6 non-oriented edges.");
        assert_eq!(non_oriented(&out.mesh), 0);
        let rows = shells(&out.mesh);
        assert_eq!((rows[0].triangles, rows[1].triangles), (4, 8));
        assert!(status(&out.mesh).oriented);
    }

    #[test]
    fn non_oriented_splitting_keeps_non_manifold_edges_and_untouched_corners() {
        // Two cubes along an edge, one with a flipped triangle away from that edge, and a third
        // cube at a corner: only the flipped triangle comes off.
        let mut a = cube();
        flip(&mut a, 2);
        let m = glued(&[
            &a,
            &make_box([20.0, 20.0, 0.0], [40.0, 40.0, 20.0]),
            &make_box([-20.0; 3], [0.0; 3]),
        ]);
        let out = split_no(&m);
        assert_eq!(out.message, "Split 3 non-oriented edges.");
        assert_eq!(non_manifold(&out.mesh).len(), 1);
        // The three cubes and the triangle that came off: shells part at the edge and the corner.
        assert_eq!(shells(&out.mesh).len(), 4);
    }

    #[test]
    fn a_mobius_seam_splits() {
        // A Möbius strip's seam edge joins two triangles whose sides run the same way; its ends
        // are on the border, so it comes apart and the strip becomes a twisted band.
        let n = 8u32;
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
            let (t, b) = (2 * i, 2 * i + 1);
            let (nt, nb) = if i + 1 < n {
                (2 * i + 2, 2 * i + 3)
            } else {
                (1, 0)
            };
            indices.extend_from_slice(&[t, b, nb, t, nb, nt]);
        }
        let m = Mesh { positions, indices };
        assert_eq!(non_oriented(&m), 1);
        let out = split_no(&m);
        assert_eq!(out.message, "Split 1 non-oriented edge.");
        assert_eq!(non_oriented(&out.mesh), 0);
        assert!(status(&out.mesh).oriented);
    }

    #[test]
    fn a_non_oriented_edge_held_by_non_manifold_edges_stays_with_a_warning() {
        // A (0, 1, 2) and B (0, 1, 3) run along edge 0-1 the same way. Round vertex 0, C joins A
        // through the consistent edge 0-2 and B through the non-manifold edge 0-3 (E is its
        // third side); round vertex 1, D does the same through 1-2 and the non-manifold 1-3 (F).
        // Non-manifold edges stay whole, so A and B stay in one fan at both ends.
        let positions = vec![
            0.0, 0.0, 0.0, // 0
            4.0, 0.0, 0.0, // 1
            1.0, 3.0, 0.0, // 2
            2.0, 1.0, 3.0, // 3
            -1.0, 2.0, 4.0, // 4
            5.0, 2.0, 4.0, // 5
        ];
        let indices = vec![
            0, 1, 2, // A
            0, 1, 3, // B
            0, 2, 3, // C
            1, 3, 2, // D
            0, 3, 4, // E
            1, 3, 5, // F
        ];
        let m = Mesh { positions, indices };
        assert_eq!(non_oriented(&m), 1);
        assert_eq!(non_manifold(&m).len(), 2);
        let out = split_no(&m);
        assert!(!out.changed);
        assert_eq!(out.message, "No non-oriented edges could be split.");
        assert_eq!(
            out.warnings,
            vec![
                "1 non-oriented edge could not be split, because its triangles are joined \
                 through other edges round both ends."
            ]
        );
        // Splitting the non-manifold edges first lets it come apart.
        let first = split_nm(&m);
        assert!(first.changed);
        let out = split_no(&first.mesh);
        assert!(out.changed && out.warnings.is_empty(), "{out:?}");
        assert_eq!(non_oriented(&out.mesh), 0);
    }

    #[test]
    fn progress_rises_and_cancel_stops() {
        let m = glued(&[&cube(), &make_box([20.0, 20.0, 0.0], [40.0, 40.0, 20.0])]);
        let mut f = cube();
        flip(&mut f, 3);
        for (mesh, action) in [
            (
                &m,
                split_non_manifold as fn(&Mesh, &[u32], &mut Control) -> _,
            ),
            (&f, split_non_oriented),
        ] {
            let never = AtomicBool::new(false);
            let mut seen = Vec::new();
            {
                let mut control = Control::new(&never, |p| seen.push(p));
                assert!(action(mesh, &[], &mut control).unwrap().changed);
            }
            assert!(seen.windows(2).all(|w| w[1] >= w[0]), "{seen:?}");
            assert_eq!(seen.last(), Some(&1.0));
            let stop = AtomicBool::new(true);
            let mut control = Control::new(&stop, |_| {});
            assert_eq!(action(mesh, &[], &mut control), Err(RepairError::Cancelled));
        }
    }

    /// Timings on a 1,002,528-triangle torus beside 8,000 small boxes in rows, touching along
    /// edges and at corners, every 997th triangle turned; then on the real models listed in
    /// TRICKY3D_MODELS (paths separated by `;`):
    /// `cargo test --release --lib repair::split_edges -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing() {
        let big = torus(40.0, 15.0, 708, 708);
        let mut parts = vec![big];
        for i in 0..4_000 {
            let (x, y) = ((i % 100) as f32, (i / 100) as f32 * 2.0);
            // Pairs of boxes along an edge on even i, at a corner on odd i; each pair also
            // touches the next one along an edge.
            let dy = if i % 2 == 0 { 0.0 } else { 0.5 };
            parts.push(make_box([x, y, 80.0], [x + 0.5, y + 0.5, 80.5]));
            parts.push(make_box(
                [x + 0.5, y + 0.5, 80.0 + dy],
                [x + 1.0, y + 1.0, 80.5 + dy],
            ));
        }
        let refs: Vec<&Mesh> = parts.iter().collect();
        let mut m = glued(&refs);
        for t in (0..m.triangle_count()).step_by(997) {
            flip(&mut m, t);
        }
        time_one("torus + 4,000 glued box pairs", &m);
        for (name, mesh) in crate::repair::flipped::testing::real_models() {
            time_one(&name, &mesh);
        }
    }

    fn time_one(name: &str, m: &Mesh) {
        for (label, action) in [
            (
                "split non-manifold",
                split_non_manifold as fn(&Mesh, &[u32], &mut Control) -> _,
            ),
            ("split non-oriented", split_non_oriented),
        ] {
            let started = Instant::now();
            let out = silent(|c| action(m, &[], c)).unwrap();
            let took = started.elapsed().as_secs_f64();
            let started = Instant::now();
            let again = silent(|c| action(&out.mesh, &[], c)).unwrap();
            let second = started.elapsed().as_secs_f64();
            println!(
                "{name}: {} triangles; {label} {:.0} ms ({}{}), again {:.0} ms ({})",
                m.triangle_count(),
                took * 1000.0,
                out.message,
                out.warnings
                    .iter()
                    .map(|w| format!(" {w}"))
                    .collect::<String>(),
                second * 1000.0,
                again.message
            );
            assert!(!again.changed);
            if label == "split non-manifold" {
                let (b, s) = (status(m), status(&out.mesh));
                assert_eq!(s.non_manifold_edges, 0);
                println!(
                    "  before: {} shells, {} non-manifold, {} border edges, {} holes; after: {} \
                     shells, {} border edges, {} holes",
                    b.shells,
                    b.non_manifold_edges,
                    b.border_edges,
                    b.holes,
                    s.shells,
                    s.border_edges,
                    s.holes
                );
            }
        }
    }
}
