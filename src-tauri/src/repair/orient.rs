//! Which way each triangle should face. Shared by the Status tab's "Inv. orientation" count and
//! Fix flipped triangles; they differ only for closed shells inside other closed shells
//! (`Nested`).

use super::bvh::{self, Aabb, Bvh};
use super::exact::vertex;
use super::topology::{Edges, NONE, Patches, side_ends};
use super::winding::nesting_depths;
use super::{Control, RepairResult};
use crate::mesh::{Mesh, cross, dot, sub};

/// Which way a closed shell inside another closed shell should face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nested {
    /// Outward, like every other closed shell (Fix flipped triangles, see `inverted`).
    Outward,
    /// The way most of its triangles face: an inward cavity and an outward embedded part are both
    /// as meant (the Status count; the reference tool shows 0 inverted triangles for a hollow
    /// part whose cavity faces inward).
    AsMost,
}

/// Triangles between cancel checks.
const CHECK_EVERY: usize = 1 << 16;

/// Per triangle: true when it faces the wrong way. Within each orientation patch the reference is
/// one of the two consistent windings: for a closed patch, the one whose volume is positive (every
/// closed shell faces outward, as the design asks); for an open patch, the winding most of its
/// triangles have (a tie keeps the patch's first triangle's).
///
/// With `Nested::Outward`, closed shells inside other closed shells face outward too. In real
/// models they are almost always parts embedded in other parts by a CAD merge, not cavities: on
/// the developer's raw files, treating them as cavities counted 111,816 triangles of 332 embedded
/// parts as inverted where 12 were. A hollow part's inward-facing inner shell is turned outward.
/// With `Nested::AsMost` such a shell takes its majority winding instead, which counts neither the
/// embedded parts nor an inward cavity.
///
/// A closed patch with no volume at all (a double-sided sheet) keeps its first triangle's winding.
/// A patch whose triangles cannot all agree (it has a conflict edge, such as a Möbius strip) has no
/// meaningful volume, so it takes the majority winding like an open one.
pub fn inverted(
    mesh: &Mesh,
    edges: &Edges,
    patches: &Patches,
    nested: Nested,
    control: &mut Control,
) -> RepairResult<Vec<bool>> {
    let count = patches.count;
    let torn = torn_patches(mesh, edges, patches);
    let solid = |p: usize| patches.closed[p] && !torn[p];
    let mut size = vec![0usize; count];
    let mut agree = vec![0usize; count];
    // Six times the signed volume of each closed patch with its first triangle's winding, measured
    // from that triangle's first corner so a small shell far from the origin keeps its precision,
    // summed with Neumaier's compensation (sum, correction), and the sum of the terms' sizes.
    let mut origin = vec![NONE; count];
    let mut volume = vec![(0.0f64, 0.0f64); count];
    let mut magnitude = vec![0.0f64; count];
    for (t, c) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        if t % CHECK_EVERY == 0 {
            control.check()?;
        }
        let p = patches.patch_of[t] as usize;
        let flipped = patches.flip[t];
        size[p] += 1;
        agree[p] += usize::from(!flipped);
        if solid(p) {
            if origin[p] == NONE {
                origin[p] = c[0];
            }
            let o = mesh.point(origin[p]);
            let [a, b, d] = c.map(|v| sub(mesh.point(v), o));
            let v = dot(a, cross(b, d));
            add(&mut volume[p], if flipped { -v } else { v });
            magnitude[p] += v.abs();
        }
    }
    // A flat patch (a double-sided sheet) has no inside: its volume is zero, or rounding noise
    // far below the size of its terms. It keeps its first triangle's winding.
    let volume: Vec<f64> = volume
        .iter()
        .zip(&magnitude)
        .map(|(&(sum, correction), &size)| {
            let v = sum + correction;
            if v.abs() <= 1e-12 * size { 0.0 } else { v }
        })
        .collect();
    // Per patch: true when its first triangle's winding is the reference.
    let majority = |p: usize| 2 * agree[p] >= size[p];
    let mut keep: Vec<bool> = (0..count)
        .map(|p| {
            if solid(p) {
                volume[p] >= 0.0
            } else {
                majority(p)
            }
        })
        .collect();
    if nested == Nested::AsMost {
        // Only closed shells whose volume and majority disagree can change.
        let differ: Vec<usize> = (0..count)
            .filter(|&p| solid(p) && keep[p] != majority(p))
            .collect();
        for p in nested_patches(mesh, patches, &solid, &differ, control)? {
            keep[p] = majority(p);
        }
    }
    Ok(patches
        .flip
        .iter()
        .zip(&patches.patch_of)
        .map(|(&flip, &p)| flip == keep[p as usize])
        .collect())
}

/// The patches of `candidates` that lie inside another closed patch (`solid`), as
/// winding::nesting_depths decides it over the candidates and the closed patches whose bounding
/// box strictly holds one of theirs.
fn nested_patches(
    mesh: &Mesh,
    patches: &Patches,
    solid: &impl Fn(usize) -> bool,
    candidates: &[usize],
    control: &mut Control,
) -> RepairResult<Vec<usize>> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    control.check()?;
    let mut boxes = vec![bvh::EMPTY; patches.count];
    for (t, c) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        let p = patches.patch_of[t] as usize;
        if solid(p) {
            for &v in c {
                bvh::include(&mut boxes[p], vertex(mesh, v));
            }
        }
    }
    let solids: Vec<usize> = (0..patches.count).filter(|&p| solid(p)).collect();
    let by_box = Bvh::new(&solids.iter().map(|&p| boxes[p]).collect::<Vec<Aabb>>());
    // Candidates held by some box, then the patches holding them: their index in `lists`.
    const HOLDS: u32 = NONE - 1;
    let mut slot = vec![NONE; patches.count];
    let mut order: Vec<usize> = Vec::new();
    for &p in candidates {
        let mut held = false;
        by_box.query(&boxes[p], |k| {
            let c = solids[k as usize];
            let (inner, outer) = (&boxes[p], &boxes[c]);
            if c != p && (0..3).all(|a| outer[0][a] < inner[0][a] && inner[1][a] < outer[1][a]) {
                held = true;
                if slot[c] == NONE {
                    slot[c] = HOLDS;
                }
            }
        });
        if held {
            slot[p] = order.len() as u32;
            order.push(p);
        }
    }
    let tested = order.len();
    if tested == 0 {
        return Ok(Vec::new());
    }
    for (p, s) in slot.iter_mut().enumerate() {
        if *s == HOLDS {
            *s = order.len() as u32;
            order.push(p);
        }
    }
    let mut lists = vec![Vec::new(); order.len()];
    for (t, &p) in patches.patch_of.iter().enumerate() {
        if slot[p as usize] != NONE {
            lists[slot[p as usize] as usize].push(t as u32);
        }
    }
    let depths = nesting_depths(mesh, &lists, control)?;
    Ok((0..tested)
        .filter(|&k| depths[k] > 0)
        .map(|k| order[k])
        .collect())
}

/// Patches with an edge whose two triangles still disagree after the orientation walk.
fn torn_patches(mesh: &Mesh, edges: &Edges, patches: &Patches) -> Vec<bool> {
    let mut torn = vec![false; patches.count];
    if patches.conflicts == 0 {
        return torn;
    }
    for e in 0..edges.len() as u32 {
        if let [s, r] = *edges.sides(e) {
            let (t, u) = ((s / 3) as usize, (r / 3) as usize);
            let same = side_ends(mesh, s).0 == side_ends(mesh, r).0;
            if t != u && same == (patches.flip[t] == patches.flip[u]) {
                torn[patches.patch_of[t] as usize] = true;
            }
        }
    }
    torn
}

/// Neumaier's compensated sum: adds `x` to (sum, correction).
fn add(acc: &mut (f64, f64), x: f64) {
    let sum = acc.0 + x;
    acc.1 += if acc.0.abs() >= x.abs() {
        (acc.0 - sum) + x
    } else {
        (x - sum) + acc.0
    };
    acc.0 = sum;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_box;
    use crate::ops::concat;
    use crate::repair::silent;
    use crate::repair::topology::orientation_patches;

    fn flip(mesh: &mut Mesh, t: usize) {
        mesh.indices.swap(t * 3 + 1, t * 3 + 2);
    }

    fn inside_out(mut mesh: Mesh) -> Mesh {
        for t in 0..mesh.triangle_count() {
            flip(&mut mesh, t);
        }
        mesh
    }

    fn cube() -> Mesh {
        make_box([0.0; 3], [20.0; 3])
    }

    fn wrong(mesh: &Mesh) -> Vec<usize> {
        let edges = Edges::new(mesh);
        let patches = orientation_patches(mesh, &edges);
        let inverted =
            silent(|control| inverted(mesh, &edges, &patches, Nested::Outward, control)).unwrap();
        assert_eq!(inverted.len(), mesh.triangle_count());
        (0..inverted.len()).filter(|&t| inverted[t]).collect()
    }

    /// The Status tab's count (`Nested::AsMost`).
    fn counted(mesh: &Mesh) -> Vec<usize> {
        let edges = Edges::new(mesh);
        let patches = orientation_patches(mesh, &edges);
        let inverted =
            silent(|control| inverted(mesh, &edges, &patches, Nested::AsMost, control)).unwrap();
        (0..inverted.len()).filter(|&t| inverted[t]).collect()
    }

    #[test]
    fn a_cube_faces_outward() {
        assert!(wrong(&cube()).is_empty());
        assert!(wrong(&Mesh::default()).is_empty());
    }

    #[test]
    fn turned_triangles_of_a_closed_shell_are_inverted() {
        let mut m = cube();
        for t in [2, 7, 11] {
            flip(&mut m, t);
        }
        assert_eq!(wrong(&m), vec![2, 7, 11]);
        // Also when the patch's first triangle is one of them, and when most of a closed shell is
        // turned: the volume decides, not the majority.
        let mut m = cube();
        for t in [0, 5, 9] {
            flip(&mut m, t);
        }
        assert_eq!(wrong(&m), vec![0, 5, 9]);
        let mut m = cube();
        for t in 0..7 {
            flip(&mut m, t);
        }
        assert_eq!(wrong(&m), (0..7).collect::<Vec<_>>());
    }

    #[test]
    fn an_inside_out_cube_is_all_inverted() {
        assert_eq!(wrong(&inside_out(cube())), (0..12).collect::<Vec<_>>());
    }

    #[test]
    fn every_closed_shell_faces_outward_even_inside_another() {
        let outer = cube();
        let inner = make_box([5.0; 3], [10.0; 3]);
        // A part embedded in another, facing out: nothing to turn.
        let embedded = concat(&[&outer, &inner]).unwrap();
        assert!(wrong(&embedded).is_empty());
        let embedded = concat(&[&inner, &outer]).unwrap();
        assert!(wrong(&embedded).is_empty());
        // An inward-facing inner shell (a hollow part's cavity) counts as inverted.
        let hollow = concat(&[&outer, &inside_out(inner)]).unwrap();
        assert_eq!(wrong(&hollow), (12..24).collect::<Vec<_>>());
        // Three deep: all of them face out.
        let c = make_box([5.5, 6.5, 7.5], [9.0; 3]);
        let three = concat(&[&cube(), &make_box([2.0; 3], [18.0; 3]), &c]).unwrap();
        assert!(wrong(&three).is_empty());
    }

    #[test]
    fn the_status_takes_a_nested_shell_as_most_of_it_faces() {
        let outer = cube();
        let inner = make_box([5.0; 3], [10.0; 3]);
        // A hollow part's inward cavity and an embedded part facing out are both as meant.
        let hollow = concat(&[&outer, &inside_out(inner.clone())]).unwrap();
        assert!(counted(&hollow).is_empty());
        assert!(counted(&concat(&[&outer, &inner]).unwrap()).is_empty());
        // A cavity with two triangles turned outward: those two (one of them the cavity's first).
        let mut turned = hollow.clone();
        for t in [12, 17] {
            flip(&mut turned, t);
        }
        assert_eq!(counted(&turned), vec![12, 17]);
        // A shell on its own, or beside another, still faces outward; so does the container.
        let far = concat(&[&cube(), &inside_out(make_box([50.0; 3], [60.0; 3]))]).unwrap();
        assert_eq!(counted(&far), (12..24).collect::<Vec<_>>());
        let both = concat(&[&inside_out(outer), &inner]).unwrap();
        assert_eq!(counted(&both), (0..12).collect::<Vec<_>>());
        assert_eq!(counted(&inside_out(cube())), (0..12).collect::<Vec<_>>());
    }

    #[test]
    fn interpenetrating_and_separate_cubes_all_face_out() {
        let two = concat(&[&cube(), &make_box([10.0, 0.0, 0.0], [30.0, 20.0, 20.0])]).unwrap();
        assert!(wrong(&two).is_empty());
        let far = concat(&[&cube(), &inside_out(make_box([50.0; 3], [60.0; 3]))]).unwrap();
        assert_eq!(wrong(&far), (12..24).collect::<Vec<_>>());
    }

    #[test]
    fn a_tiny_shell_far_from_the_origin_keeps_its_sign() {
        // A cube one f32 step wide (about 1 µm, 1e-9 mm³) 12 m out, and a 1 mm cube 10 km out:
        // measured from the origin, rounding would swamp both volumes.
        let m = make_box([1e7, 1e7, 1e7], [1e7 + 1.0, 1e7 + 1.0, 1e7 + 1.0]);
        let mut tiny = make_box([12345.0; 3], [12345.001; 3]);
        assert!(crate::mesh::stats(&tiny).volume.abs() > 0.0);
        assert!(wrong(&tiny).is_empty());
        flip(&mut tiny, 4);
        assert_eq!(wrong(&tiny), vec![4]);
        assert_eq!(wrong(&inside_out(m)), (0..12).collect::<Vec<_>>());
    }

    #[test]
    fn open_patches_follow_the_majority_and_ties_keep_the_first() {
        let mut open = cube();
        open.indices.truncate(30);
        for t in [3, 8] {
            flip(&mut open, t);
        }
        assert_eq!(wrong(&open), vec![3, 8]);
        // Two triangles facing opposite ways: the first one's winding wins.
        let pair = Mesh {
            positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0],
            indices: vec![0, 1, 2, 1, 2, 3],
        };
        assert_eq!(wrong(&pair), vec![1]);
    }

    #[test]
    fn a_double_sided_sheet_and_a_mobius_strip_get_an_answer() {
        // Two triangles back to back form a closed patch with no volume: kept as they are.
        let sheet = Mesh {
            positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            indices: vec![0, 1, 2, 0, 2, 1],
        };
        assert!(wrong(&sheet).is_empty());
        // A Möbius strip cannot agree everywhere; most triangles keep their winding.
        let n = 10u32;
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
        let strip = Mesh { positions, indices };
        assert!(wrong(&strip).len() * 2 <= strip.triangle_count());
    }

    #[test]
    fn turning_the_inverted_triangles_leaves_none() {
        // What Fix flipped triangles does, twice: the second time nothing may be inverted.
        let outer = cube();
        let inner = make_box([5.0; 3], [10.0; 3]);
        let mut scrambled = concat(&[&outer, &inner, &make_box([30.0; 3], [33.0; 3])]).unwrap();
        for t in [0, 3, 4, 13, 14, 15, 16, 17, 30] {
            flip(&mut scrambled, t);
        }
        let mut open = cube();
        open.indices.truncate(27);
        for t in [1, 5, 6] {
            flip(&mut open, t);
        }
        // A Möbius strip with a tab glued on against its winding as triangle 0.
        let n = 7u32;
        let mut positions = Vec::new();
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let (c, s) = ((a / 2.0).cos(), (a / 2.0).sin());
            for w in [1.0f32, -1.0] {
                let r = 4.0 + w * c;
                positions.extend_from_slice(&[r * a.cos(), r * a.sin(), w * s]);
            }
        }
        positions.extend_from_slice(&[4.0, 0.0, -3.0]);
        let mut indices = vec![1, 3, 2 * n];
        for i in 0..n {
            let (t, b) = (2 * i, 2 * i + 1);
            let (nt, nb) = if i + 1 < n {
                (2 * i + 2, 2 * i + 3)
            } else {
                (1, 0)
            };
            indices.extend_from_slice(&[t, b, nb, t, nb, nt]);
        }
        let strip = Mesh { positions, indices };
        for mesh in [scrambled, open, strip] {
            let mut m = mesh.clone();
            for t in wrong(&m) {
                flip(&mut m, t);
            }
            assert!(wrong(&m).is_empty());
        }
    }

    #[test]
    fn cancelling_stops_the_work() {
        let m = cube();
        let edges = Edges::new(&m);
        let patches = orientation_patches(&m, &edges);
        let stop = std::sync::atomic::AtomicBool::new(true);
        let mut control = Control::new(&stop, |_| {});
        assert_eq!(
            inverted(&m, &edges, &patches, Nested::Outward, &mut control),
            Err(crate::repair::RepairError::Cancelled)
        );
    }
}
