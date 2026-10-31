//! Lifting straight up (+Z). A solid lifted up hits another when some point of the other lies
//! directly above a point of it: over every place where two triangles overlap seen from above,
//! the height difference of their planes is linear, so it is enough to look at the corners of the
//! overlap. Touching (no height difference) is not a hit.
//!
//! The interlocking test lifts parts out one after another, every part that is free at each round,
//! until none is left or the rest hold each other. Z-removability looks inside one part: its bodies
//! (shells joined where they touch, a cavity kept with the body around it) are each lifted alone
//! while the others stay; a body that would hit another is trapped.

use rayon::prelude::*;
use serde::Serialize;

use crate::arrange::collide::{Body, closer_than, collide};
use crate::arrange::{Aabb, V3, World, sub};
use crate::mesh::Mesh;
use crate::repair::bvh::{Bvh, boxes_of, overlap};
use crate::repair::topology::{Edges, Link, components};
use crate::repair::{Control, RepairResult};

/// A solid ready for lift tests.
pub struct Solid {
    world: World,
    boxes: Vec<Aabb>,
    bvh: Bvh,
    bounds: Option<Aabb>,
}

impl Solid {
    pub fn new(world: World) -> Solid {
        let boxes = boxes_of(&world.points, &world.triangles);
        let bvh = Bvh::new(&boxes);
        let bounds = bvh.bounds();
        Solid {
            world,
            boxes,
            bvh,
            bounds,
        }
    }
}

type P2 = [f64; 2];

fn cross2(o: P2, a: P2, b: P2) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// The part of convex polygon `poly` left of every edge of anticlockwise triangle `clip`
/// (Sutherland–Hodgman).
fn clip(mut poly: Vec<P2>, clip: [P2; 3]) -> Vec<P2> {
    for k in 0..3 {
        let (a, b) = (clip[k], clip[(k + 1) % 3]);
        if poly.is_empty() {
            break;
        }
        let input = std::mem::take(&mut poly);
        for i in 0..input.len() {
            let (p, q) = (input[i], input[(i + 1) % input.len()]);
            let (sp, sq) = (cross2(a, b, p), cross2(a, b, q));
            if sp >= 0.0 {
                poly.push(p);
            }
            if (sp >= 0.0) != (sq >= 0.0) {
                let t = sp / (sp - sq);
                poly.push([p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])]);
            }
        }
    }
    poly
}

/// The triangle seen from above, anticlockwise, and its plane's height as a function of x and y;
/// None for an upright (or flat) triangle, which covers no area from above.
fn from_above(t: [V3; 3]) -> Option<([P2; 3], impl Fn(P2) -> f64)> {
    let [a, b, c] = t;
    let n = crate::arrange::cross(sub(b, a), sub(c, a));
    let size = crate::arrange::len(n);
    if size == 0.0 || n[2].abs() <= 1e-12 * size {
        return None;
    }
    let mut flat = [[a[0], a[1]], [b[0], b[1]], [c[0], c[1]]];
    if cross2(flat[0], flat[1], flat[2]) < 0.0 {
        flat.swap(1, 2);
    }
    let height = move |p: P2| a[2] - (n[0] * (p[0] - a[0]) + n[1] * (p[1] - a[1])) / n[2];
    Some((flat, height))
}

/// True when lifting `p` straight up would hit `q`.
pub fn blocks(p: &Solid, q: &Solid) -> bool {
    let (Some(bp), Some(bq)) = (p.bounds, q.bounds) else {
        return false;
    };
    let apart_xy = (0..2).any(|k| bq[1][k] <= bp[0][k] || bq[0][k] >= bp[1][k]);
    if apart_xy || bq[1][2] <= bp[0][2] {
        return false;
    }
    let scale = (0..3).fold(0.0f64, |m, k| {
        m.max(bp[1][k] - bp[0][k]).max(bq[1][k] - bq[0][k])
    });
    let eps = 1e-9 * scale.max(1.0);
    let area_eps = eps * eps;
    p.world.triangles.par_iter().enumerate().any(|(i, &t)| {
        let tb = p.boxes[i];
        // Only what is higher than the triangle's lowest point can be above it.
        let reach = [
            [tb[0][0], tb[0][1], tb[0][2]],
            [tb[1][0], tb[1][1], bq[1][2]],
        ];
        if !overlap(&reach, &bq) {
            return false;
        }
        let Some((tf, tz)) = from_above(p.world.corners(t)) else {
            return false;
        };
        let mut hit = false;
        q.bvh.query(&reach, |j| {
            if hit {
                return;
            }
            let Some((uf, uz)) = from_above(q.world.corners(q.world.triangles[j as usize])) else {
                return;
            };
            let shared = clip(tf.to_vec(), uf);
            if shared.len() < 3 {
                return;
            }
            let area: f64 = (1..shared.len() - 1)
                .map(|k| cross2(shared[0], shared[k], shared[k + 1]))
                .sum::<f64>()
                / 2.0;
            if area <= area_eps {
                return;
            }
            hit = shared.iter().any(|&c| uz(c) - tz(c) > eps);
        });
        hit
    })
}

/// The triangles `list` of `world` with only the points they use.
fn sub_world(world: &World, list: &[u32]) -> World {
    let mut new_index = std::collections::HashMap::new();
    let mut points = Vec::new();
    let triangles = list
        .iter()
        .map(|&t| {
            world.triangles[t as usize].map(|v| {
                *new_index.entry(v).or_insert_with(|| {
                    points.push(world.points[v as usize]);
                    (points.len() - 1) as u32
                })
            })
        })
        .collect();
    World { points, triangles }
}

/// `InterlockResult` in src/analysis/types.ts: the parts in the order they can be lifted out
/// (each round's together), and those that cannot, as indices of the parts given.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Interlock {
    pub rounds: Vec<Vec<usize>>,
    pub trapped: Vec<usize>,
}

/// Which solid blocks which: `blocked[i]` lists the solids that lifting solid i would hit.
fn blockers(solids: &[Solid], control: &mut Control) -> RepairResult<Vec<Vec<usize>>> {
    let n = solids.len();
    let mut blocked = vec![Vec::new(); n];
    let pairs = n * n.saturating_sub(1);
    let mut done = 0;
    for (i, p) in solids.iter().enumerate() {
        for (j, q) in solids.iter().enumerate() {
            if i == j {
                continue;
            }
            control.check()?;
            if blocks(p, q) {
                blocked[i].push(j);
            }
            done += 1;
            control.progress(done as f64 / pairs.max(1) as f64);
        }
    }
    Ok(blocked)
}

/// Lifts the solids out round by round.
pub fn interlock(solids: &[Solid], control: &mut Control) -> RepairResult<Interlock> {
    let blocked = blockers(solids, control)?;
    let mut left: Vec<bool> = vec![true; solids.len()];
    let mut rounds = Vec::new();
    loop {
        let free: Vec<usize> = (0..solids.len())
            .filter(|&i| left[i] && blocked[i].iter().all(|&j| !left[j]))
            .collect();
        if free.is_empty() {
            break;
        }
        for &i in &free {
            left[i] = false;
        }
        rounds.push(free);
    }
    Ok(Interlock {
        rounds,
        trapped: (0..solids.len()).filter(|&i| left[i]).collect(),
    })
}

/// `RemovabilityResult` in src/analysis/types.ts.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Removability {
    pub removable: bool,
    pub bodies: u64,
    pub trapped_bodies: u64,
    /// mm² of surface of the trapped bodies.
    pub trapped_area: f64,
}

/// Z-removability of one placed part, with a class per triangle: 1 for a body that lifts out
/// freely, 2 for a trapped one.
pub fn removability(
    mesh: &Mesh,
    world: &World,
    control: &mut Control,
) -> RepairResult<(Removability, Vec<u8>)> {
    let edges = Edges::new(mesh);
    let (shell_of, shells) = components(mesh, &edges, Link::Shell);
    control.progress(0.05);
    // Each shell's triangles, signed volume and box.
    let mut members: Vec<Vec<u32>> = vec![Vec::new(); shells];
    for (t, &s) in shell_of.iter().enumerate() {
        members[s as usize].push(t as u32);
    }
    let worlds: Vec<World> = members.iter().map(|list| sub_world(world, list)).collect();
    let volumes: Vec<f64> = worlds.iter().map(World::volume).collect();
    let bounds: Vec<Option<Aabb>> = worlds.iter().map(World::bounds).collect();
    // Bodies: shells joined where they touch or cross, a cavity with the smallest body round it.
    let mut parent: Vec<usize> = (0..shells).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let bodies_of: Vec<Body> = worlds.iter().cloned().map(Body::new).collect();
    let inside = |a: &Aabb, b: &Aabb| (0..3).all(|k| a[0][k] >= b[0][k] && a[1][k] <= b[1][k]);
    for i in 0..shells {
        control.check()?;
        let Some(bi) = bounds[i] else { continue };
        if volumes[i] < 0.0 {
            let host = (0..shells)
                .filter(|&j| {
                    j != i && volumes[j] > 0.0 && bounds[j].is_some_and(|bj| inside(&bi, &bj))
                })
                .min_by(|&a, &b| volumes[a].total_cmp(&volumes[b]));
            if let Some(j) = host {
                let (ri, rj) = (root(&mut parent, i), root(&mut parent, j));
                parent[ri] = rj;
            }
            continue;
        }
        for j in i + 1..shells {
            if volumes[j] <= 0.0 || bounds[j].is_none() {
                continue;
            }
            if collide(&bodies_of[i], &bodies_of[j])
                || closer_than(&bodies_of[i], &bodies_of[j], 1e-6).is_some()
            {
                let (ri, rj) = (root(&mut parent, i), root(&mut parent, j));
                parent[ri] = rj;
            }
        }
        control.progress(0.05 + 0.25 * (i + 1) as f64 / shells as f64);
    }
    let mut body_of_shell = vec![usize::MAX; shells];
    let mut bodies: Vec<Vec<usize>> = Vec::new();
    for s in 0..shells {
        let r = root(&mut parent, s);
        if body_of_shell[r] == usize::MAX {
            body_of_shell[r] = bodies.len();
            bodies.push(Vec::new());
        }
        body_of_shell[s] = body_of_shell[r];
        bodies[body_of_shell[s]].push(s);
    }
    let solids: Vec<Solid> = bodies
        .iter()
        .map(|list| {
            let triangles: Vec<u32> = list
                .iter()
                .flat_map(|&s| members[s].iter().copied())
                .collect();
            Solid::new(sub_world(world, &triangles))
        })
        .collect();
    let blocked = blockers(&solids, &mut control.sub(0.3, 1.0))?;
    let trapped: Vec<bool> = blocked.iter().map(|b| !b.is_empty()).collect();
    let mut classes = vec![1u8; world.triangles.len()];
    let mut trapped_area = 0.0;
    for (t, &s) in shell_of.iter().enumerate() {
        if trapped[body_of_shell[s as usize]] {
            classes[t] = 2;
            trapped_area += world.normal_area(world.triangles[t]).1;
        }
    }
    let trapped_bodies = trapped.iter().filter(|&&t| t).count() as u64;
    Ok((
        Removability {
            removable: trapped_bodies == 0,
            bodies: bodies.len() as u64,
            trapped_bodies,
            trapped_area,
        },
        classes,
    ))
}
