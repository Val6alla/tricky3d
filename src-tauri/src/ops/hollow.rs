//! Hollow (Modify › Hollow, Home › Hollow): a part made hollow with drain holes, in one panel. The
//! cavity is Generate shell's inner offset (src/ops/shell.rs `cavity`), so the hollow part is the
//! part with that cavity turned inside out, closed, oriented and free of crossings by
//! construction.
//!
//! A drain hole runs along the outer surface's normal where it was placed, from just outside the
//! part through the wall into the cavity: a solid of revolution (`hole_solid`) as wide as its
//! diameter where it meets the cavity, wider towards the outside by its taper, with its rim rounded
//! by the flare. A hole is checked first (`check_hole`): it must reach the cavity before it leaves
//! the part, and keep clear of the other holes.
//!
//! Cutting the holes is a Boolean done where they are only (`cut`): the hollow part's triangles near
//! the holes and the holes' own triangles go through the exact arrangement of Wrap part surface
//! (src/repair/arrangement.rs), which splits every triangle along the curves where others cross
//! it; each piece is then kept or dropped by which side of the other solid it lies on. The rest of
//! the part is left as it was. A plug is the same cut kept the other way: a hole narrower by the
//! clearance, where it lies inside the wall. Every result is checked as Generate shell checks its
//! own: closed, oriented, no new crossings; should rounding the new points to the mesh's precision
//! make two triangles cross, the cut runs again with the holes turned a little about their axes.

use serde::{Deserialize, Serialize};

use super::shell::field::Surface;
use crate::mesh::{Mesh, weld_indexed};
use crate::repair::bvh::{self, Aabb, Bvh};
use crate::repair::exact::{self, P3, add, cross, dot, scale, sub};
use crate::repair::{Control, RepairError, RepairResult, arrangement};

/// `HollowSettings` in src/hollow/types.ts, mm.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HollowSettings {
    /// Wall thickness.
    pub thickness: f64,
    /// The grid step: smaller is finer and slower.
    pub accuracy: f64,
    /// 0 (off) to 3: how much the cavity's surface is smoothed.
    pub smoothness: u8,
}

impl HollowSettings {
    pub fn problem(&self) -> Option<String> {
        if self.smoothness > 3 {
            return Some("Pick a smoothness from Off to High.".into());
        }
        super::shell::settings_problem(&self.shell())
            .map(|p| p.replace("shell thickness", "wall thickness"))
    }

    fn shell(&self) -> super::shell::ShellSettings {
        super::shell::ShellSettings {
            kind: super::shell::ShellKind::HollowPart,
            thickness: self.thickness,
            accuracy: self.accuracy,
            smooth: self.smoothness > 0,
        }
    }

    /// Pairs of smoothing steps for the cavity.
    fn smooth_pairs(&self) -> usize {
        [0, 2, 4, 8][self.smoothness.min(3) as usize]
    }
}

/// `Hole` in src/hollow/types.ts: where a drain hole goes and its shape, mm and degrees.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hole {
    /// The point on the part's outer surface it starts at.
    pub at: P3,
    /// The outer surface's outward normal there: the hole runs the other way.
    pub normal: P3,
    /// Diameter where it meets the cavity.
    pub diameter: f64,
    /// How far its side leans out from its axis, wider towards the outside (0: a straight hole).
    pub taper: f64,
    /// Radius of the rounding of its rim on the outer surface (0: a sharp rim).
    pub flare: f64,
}

pub const MIN_DIAMETER: f64 = 0.3;
pub const MAX_DIAMETER: f64 = 50.0;
pub const MAX_TAPER: f64 = 45.0;
pub const MAX_FLARE: f64 = 10.0;

impl Hole {
    pub fn problem(&self) -> Option<String> {
        if !(self.diameter.is_finite() && (MIN_DIAMETER..=MAX_DIAMETER).contains(&self.diameter)) {
            return Some(format!(
                "Enter a hole diameter from {MIN_DIAMETER} to {MAX_DIAMETER} mm."
            ));
        }
        if !(self.taper.is_finite() && (0.0..=MAX_TAPER).contains(&self.taper)) {
            return Some(format!("Enter a taper from 0° to {MAX_TAPER}°."));
        }
        if !(self.flare.is_finite() && (0.0..=MAX_FLARE).contains(&self.flare)) {
            return Some(format!("Enter a flare from 0 to {MAX_FLARE} mm."));
        }
        if !(self.at.iter().chain(&self.normal).all(|x| x.is_finite())
            && exact::length(self.normal) > 0.0)
        {
            return Some("The hole's place is damaged.".into());
        }
        None
    }

    fn axis(&self) -> P3 {
        scale(self.normal, -1.0 / exact::length(self.normal))
    }
}

/// `HoleCheck` in src/hollow/types.ts.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HoleCheck {
    pub ok: bool,
    /// Why it would not be cut, as a sentence.
    pub reason: Option<String>,
    /// How far it runs through the wall before it reaches the cavity, mm.
    pub depth: Option<f64>,
    /// How far the cavity reaches along its axis after that, mm.
    pub room: Option<f64>,
}

impl HoleCheck {
    fn refused(reason: String) -> HoleCheck {
        HoleCheck {
            ok: false,
            reason: Some(reason),
            depth: None,
            room: None,
        }
    }
}

/// The part and its cavity, for placing and checking holes.
pub struct Scene {
    part: Surface,
    cavity: Option<Surface>,
    /// The part's box diagonal: further than any hole could run.
    reach: f64,
    pub settings: HollowSettings,
}

impl Scene {
    pub fn new(part: &Mesh, cavity: Option<&Mesh>, settings: HollowSettings) -> Scene {
        let b = crate::mesh::bounds(part).unwrap_or([[0.0; 3]; 2]);
        let reach = (0..3)
            .map(|k| (b[1][k] - b[0][k]) as f64)
            .map(|d| d * d)
            .sum::<f64>()
            .sqrt()
            + 1.0;
        Scene {
            part: Surface::new(part),
            cavity: cavity.filter(|c| c.triangle_count() > 0).map(Surface::new),
            reach,
            settings,
        }
    }
}

/// The radius of a hole `s` mm in from where it starts (negative: outside the part), for a hole
/// whose wall is `depth` mm deep: the diameter's half at the cavity, wider by the taper towards the
/// outside, and round its rim by the flare.
fn radius_at(hole: &Hole, depth: f64, s: f64) -> f64 {
    let r = hole.diameter / 2.0
        + (depth - s.clamp(0.0, depth)).max(0.0) * hole.taper.to_radians().tan();
    let f = hole.flare.min(depth.max(0.0));
    let s = s.max(0.0);
    if f > 0.0 && s < f {
        r + f - (f * f - (f - s) * (f - s)).max(0.0).sqrt()
    } else {
        r
    }
}

/// Checks hole `index` of `holes`: it must reach the cavity before it comes out of the part, and keep
/// clear of the holes before it.
pub fn check_hole(scene: &Scene, holes: &[Hole], index: usize) -> HoleCheck {
    let hole = &holes[index];
    if let Some(problem) = hole.problem() {
        return HoleCheck::refused(problem);
    }
    let Some(cavity) = &scene.cavity else {
        return HoleCheck::refused(
            "There is no cavity yet: every wall of the part is thinner than twice the wall \
             thickness."
                .into(),
        );
    };
    let dir = hole.axis();
    // From just outside the surface, through it.
    let start = add(hole.at, scale(dir, -1e-3));
    let end = add(start, scale(dir, scale_len(scene.reach)));
    let length = exact::length(sub(end, start));
    let into = cavity
        .hits(start, end)
        .into_iter()
        .map(|(f, _, d)| (f * length - 1e-3, d))
        .collect::<Vec<_>>();
    // Entering the cavity: through the back of its surface (it faces out of the cavity solid).
    let Some(enter) = into.iter().position(|&(_, d)| d < 0.0) else {
        return HoleCheck::refused(
            "No cavity lies behind this point: the wall here is solid (thinner than twice the wall \
             thickness) or the hole points past the cavity."
                .into(),
        );
    };
    let depth = into[enter].0.max(0.0);
    let room = into[enter + 1..]
        .iter()
        .find(|&&(_, d)| d > 0.0)
        .map_or(0.0, |&(s, _)| s - depth);
    // Leaving the part (through the front of its surface) before that: the hole comes out again.
    let out_first = scene
        .part
        .hits(start, end)
        .into_iter()
        .map(|(f, _, d)| (f * length - 1e-3, d))
        .find(|&(s, d)| d > 0.0 && s > 1e-3);
    if out_first.is_some_and(|(s, _)| s < depth) {
        return HoleCheck::refused(
            "The hole would come out of the part again before it reaches the cavity.".into(),
        );
    }
    // Never past twice the wall and its rounding: a hole that long missed the wall it was placed on.
    let t = scene.settings.thickness;
    if depth > 4.0 * t + hole.diameter + 2.0 * scene.settings.accuracy + 2.0 {
        return HoleCheck::refused(format!(
            "The cavity is {} mm in along the hole, much further than the wall thickness: place the \
             hole where the wall is.",
            round1(depth)
        ));
    }
    let mouth = radius_at(hole, depth, 0.0);
    for (j, other) in holes[..index].iter().enumerate() {
        if other.problem().is_some() {
            continue;
        }
        let gap = exact::length(sub(other.at, hole.at));
        let other_mouth = radius_at(other, depth, 0.0);
        if gap < mouth + other_mouth + 0.2 {
            return HoleCheck::refused(format!(
                "It runs into hole {}: move it further away or make it narrower.",
                j + 1
            ));
        }
    }
    HoleCheck {
        ok: true,
        reason: None,
        depth: Some(depth),
        room: Some(room),
    }
}

fn scale_len(reach: f64) -> f64 {
    reach.max(1.0)
}

fn round1(x: f64) -> String {
    let s = format!("{:.1}", x);
    s.trim_end_matches(".0").to_string()
}

/// Segments round a hole.
const SEGMENTS: usize = 48;

/// The drain hole's solid (closed, facing out): from outside the part, where its rim's radius is
/// kept, through the wall `depth` mm deep, to a little into the cavity (less than half the `room`
/// the cavity has along the axis). `narrower` takes that much off the radius everywhere (plugs);
/// `turn` turns the segments about the axis (radians), for a second try at a cut.
pub fn hole_solid(hole: &Hole, depth: f64, room: f64, narrower: f64, turn: f64) -> Mesh {
    let axis = hole.axis();
    let helper = if axis[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let u = {
        let c = cross(axis, helper);
        scale(c, 1.0 / exact::length(c))
    };
    let v = cross(axis, u);
    let mouth = radius_at(hole, depth, 0.0);
    let outside = mouth + 0.5;
    // Far enough into the cavity that a floor at a slant to the axis still opens all round.
    let into = hole.diameter.max(1.0).min(0.45 * room.max(0.0)).max(0.05);
    // The profile: (depth along the axis, radius), outside first. No ring lies on the surfaces the
    // hole cuts (at 0 and at `depth`), where its points would meet theirs nearly head on: the side
    // runs straight from just outside the part, through the flare's rounding, to inside the cavity,
    // the taper going on past the cavity's wall.
    let edge = 0.03f64.max(0.01 * depth);
    let mut profile: Vec<(f64, f64)> = vec![(-outside, mouth), (-edge, mouth)];
    let f = hole.flare.min(depth);
    if f > 0.0 {
        for k in 1..=8 {
            // Closer together near the rim, where the rounding turns fastest.
            let s = f * (k as f64 / 8.0).powi(2);
            profile.push((s, radius_at(hole, depth, s)));
        }
    }
    let end = depth + into;
    let tan = hole.taper.to_radians().tan();
    profile.push((end, (hole.diameter / 2.0 - into * tan).max(0.05)));
    let profile: Vec<(f64, f64)> = profile
        .into_iter()
        .map(|(s, r)| (s, (r - narrower).max(0.05)))
        .collect();
    let mut positions: Vec<f32> = Vec::with_capacity(3 * (profile.len() * SEGMENTS + 2));
    for &(s, r) in &profile {
        let centre = add(hole.at, scale(axis, s));
        for j in 0..SEGMENTS {
            let a = turn + std::f64::consts::TAU * j as f64 / SEGMENTS as f64;
            let p = add(centre, add(scale(u, r * a.cos()), scale(v, r * a.sin())));
            positions.extend(p.map(|x| x as f32));
        }
    }
    let ring = |i: usize, j: usize| (i * SEGMENTS + j % SEGMENTS) as u32;
    let mut indices = Vec::new();
    // The side, then the two ends wound the same way round as the side (each side edge runs once
    // each way); which way the whole faces is settled by its volume below.
    for i in 0..profile.len() - 1 {
        for j in 0..SEGMENTS {
            indices.extend([ring(i, j), ring(i + 1, j), ring(i + 1, j + 1)]);
            indices.extend([ring(i, j), ring(i + 1, j + 1), ring(i, j + 1)]);
        }
    }
    let last = profile.len() - 1;
    for (i, first) in [(0, true), (last, false)] {
        let centre = (positions.len() / 3) as u32;
        positions.extend(add(hole.at, scale(axis, profile[i].0)).map(|x| x as f32));
        for j in 0..SEGMENTS {
            if first {
                indices.extend([centre, ring(i, j), ring(i, j + 1)]);
            } else {
                indices.extend([centre, ring(i, j + 1), ring(i, j)]);
            }
        }
    }
    let mesh = Mesh { positions, indices };
    if crate::mesh::stats(&mesh).volume < 0.0 {
        super::shell::turned(mesh)
    } else {
        mesh
    }
}

/// Up to `count` holes like `template`, at the lowest points of the cavity (as the part stands),
/// spread as far apart as they go: each runs straight up from the outer surface under its point,
/// along that surface's normal, and is kept only when it checks out.
pub fn auto_holes(scene: &Scene, count: usize, template: &Hole) -> Vec<Hole> {
    let Some(cavity) = &scene.cavity else {
        return Vec::new();
    };
    let points = cavity.points();
    if points.is_empty() || count == 0 {
        return Vec::new();
    }
    let (lo, hi) = points
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
            (lo.min(p[2]), hi.max(p[2]))
        });
    let tol = (0.05 * (hi - lo))
        .max(2.0 * scene.settings.accuracy)
        .max(0.2);
    let band: Vec<P3> = points
        .iter()
        .copied()
        .filter(|p| p[2] <= lo + tol)
        .collect();
    let centre = scale(
        band.iter().fold([0.0; 3], |m, &p| add(m, p)),
        1.0 / band.len() as f64,
    );
    let flat = |a: P3, b: P3| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
    // Farthest point sampling across the floor, starting low and far out: on a flat floor a corner,
    // on a round bottom its lowest point.
    let first = band
        .iter()
        .copied()
        .min_by(|a, b| {
            let score = |p: &P3| (p[2] - lo) - 0.1 * flat(*p, centre);
            score(a).total_cmp(&score(b))
        })
        .unwrap_or(centre);
    let mut picked: Vec<P3> = vec![first];
    let mut nearest: Vec<f64> = band.iter().map(|&p| flat(p, first)).collect();
    let mut out: Vec<Hole> = Vec::new();
    let mut tried = 0;
    let r = template.diameter / 2.0;
    loop {
        let c = *picked.last().unwrap_or(&first);
        tried += 1;
        // Moved in towards the middle of the floor, a quarter millimetre at a time, until the
        // whole hole opens into the cavity.
        let towards = flat(centre, c);
        let mut moved = 0.0;
        let found = loop {
            let inward = if towards > 0.0 {
                add(
                    c,
                    scale([centre[0] - c[0], centre[1] - c[1], 0.0], moved / towards),
                )
            } else {
                c
            };
            if let Some(hole) = under(scene, inward, template) {
                let mut all = out.clone();
                all.push(hole);
                let check = check_hole(scene, &all, all.len() - 1);
                if let (true, Some(depth)) = (check.ok, check.depth)
                    && opens_fully(scene, &hole, depth)
                {
                    break Some(hole);
                }
            }
            moved += 0.25;
            if moved > towards || moved > 4.0 * (r + 1.0) {
                break None;
            }
        };
        if let Some(hole) = found {
            out.push(hole);
        }
        if out.len() >= count || tried >= 4 * count + 8 {
            break;
        }
        // The next: the floor point farthest from every one picked, if it is far enough to make a
        // hole of its own.
        let Some((i, &d)) = nearest.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) else {
            break;
        };
        if d < template.diameter + 1.0 {
            break;
        }
        let next = band[i];
        picked.push(next);
        for (k, p) in band.iter().enumerate() {
            nearest[k] = nearest[k].min(flat(*p, next));
        }
    }
    out
}

/// True when the whole of a hole `depth` deep opens into the cavity: lines along its axis round its
/// rim (where it meets the cavity) all reach the cavity before they leave the part, within a hole's
/// width of the depth at its axis.
fn opens_fully(scene: &Scene, hole: &Hole, depth: f64) -> bool {
    let Some(cavity) = &scene.cavity else {
        return false;
    };
    let axis = hole.axis();
    let helper = if axis[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let u = {
        let c = cross(axis, helper);
        scale(c, 1.0 / exact::length(c))
    };
    let v = cross(axis, u);
    let r = hole.diameter / 2.0;
    (0..8).all(|k| {
        let a = std::f64::consts::TAU * k as f64 / 8.0;
        let start = add(
            add(hole.at, scale(axis, -1.0)),
            add(scale(u, r * a.cos()), scale(v, r * a.sin())),
        );
        let end = add(start, scale(axis, depth + 1.0 + 2.0 * hole.diameter + 1.0));
        let length = exact::length(sub(end, start));
        let enter = cavity
            .hits(start, end)
            .into_iter()
            .find(|&(_, _, d)| d < 0.0)
            .map(|(f, _, _)| f * length);
        let leave = scene
            .part
            .hits(start, end)
            .into_iter()
            .filter(|&(_, _, d)| d > 0.0)
            .map(|(f, _, _)| f * length)
            .find(|&s| s > 1.0 + 1e-3);
        matches!(enter, Some(e) if leave.is_none_or(|l| l > e))
    })
}

/// The hole straight below `p` (a point of the cavity): where a line down from it leaves the part.
fn under(scene: &Scene, p: P3, template: &Hole) -> Option<Hole> {
    let start = add(p, [0.0, 0.0, 1e-3]);
    let end = add(p, [0.0, 0.0, -scene.reach]);
    let (f, t, _) = scene
        .part
        .hits(start, end)
        .into_iter()
        .find(|&(_, _, d)| d > 0.0)?;
    let at = add(start, scale(sub(end, start), f));
    Some(Hole {
        at,
        normal: scene.part.unit_normal(t),
        ..*template
    })
}

// ---------------------------------------------------------------------------------------------
// The cut

/// Which pieces a cut keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Keep {
    /// The part without the tools (holes).
    Difference,
    /// Only what the part and the tools share (plugs).
    Intersection,
}

/// The winding number of a closed surface at `p`: how many times it wraps round `p` (1 inside,
/// 0 outside). Three rays in skew directions vote, so one that grazes an edge does not decide.
fn winding(points: &[P3], tris: &[[u32; 3]], tree: &Bvh, p: P3, reach: f64) -> i32 {
    const DIRECTIONS: [P3; 3] = [
        [0.57735, 0.57741, 0.57729],
        [-0.31623, 0.94868, 0.0011],
        [0.0013, -0.44722, -0.89442],
    ];
    let mut votes = [0i32; 3];
    for (k, d) in DIRECTIONS.iter().enumerate() {
        let end = add(p, scale(*d, reach));
        let dir = sub(end, p);
        let mut w = 0;
        tree.segment(p, end, |t| {
            let [a, b, c] = tris[t as usize].map(|v| points[v as usize]);
            if crate::repair::levelset::segment_hit(p, dir, a, b, c).is_some() {
                w += if dot(cross(sub(b, a), sub(c, a)), dir) > 0.0 {
                    1
                } else {
                    -1
                };
            }
        });
        votes[k] = w;
    }
    if votes[0] == votes[1] || votes[0] == votes[2] {
        votes[0]
    } else {
        votes[1]
    }
}

/// A closed mesh with what winding numbers need: its points, triangles and a tree over them.
struct Solid {
    points: Vec<P3>,
    tris: Vec<[u32; 3]>,
    tree: Bvh,
    reach: f64,
}

impl Solid {
    fn new(mesh: &Mesh) -> Solid {
        let points: Vec<P3> = (0..mesh.vertex_count() as u32)
            .map(|v| exact::vertex(mesh, v))
            .collect();
        let tris: Vec<[u32; 3]> = mesh.indices.as_chunks::<3>().0.to_vec();
        let tree = Bvh::new(&bvh::boxes_of(&points, &tris));
        let reach = tree
            .bounds()
            .map_or(1.0, |b| 2.0 * exact::length(sub(b[1], b[0])) + 1.0);
        Solid {
            points,
            tris,
            tree,
            reach,
        }
    }

    fn inside(&self, p: P3) -> bool {
        winding(&self.points, &self.tris, &self.tree, p, self.reach) > 0
    }
}

/// What a cut made: the mesh, and where its new triangles start (those before are the part's own,
/// untouched).
struct Cut {
    mesh: Mesh,
    first_new: usize,
}

/// `part` (closed, oriented) cut by `tools` (closed, oriented, apart from each other): only the
/// part's triangles whose boxes meet a tool's box take part. With `whole`, the part's other
/// triangles come first, as they were; without, only the pieces the cut keeps.
fn cut(
    part: &Mesh,
    part_solid: &Solid,
    tools: &[Mesh],
    keep: Keep,
    whole: bool,
    control: &mut Control,
) -> RepairResult<Result<Cut, String>> {
    let boxes: Vec<Aabb> = tools
        .iter()
        .filter_map(crate::mesh::bounds)
        .map(|b| {
            [
                [0, 1, 2].map(|k| b[0][k] as f64 - 0.01),
                [0, 1, 2].map(|k| b[1][k] as f64 + 0.01),
            ]
        })
        .collect();
    let mut near = vec![false; part.triangle_count()];
    for b in &boxes {
        part_solid.tree.query(b, |t| near[t as usize] = true);
    }
    let patch: Vec<u32> = (0..part.triangle_count() as u32)
        .filter(|&t| near[t as usize])
        .collect();
    // The local mesh: the patch's triangles, then the tools' (turned for a difference, so the hole's
    // wall faces into the hole).
    let mut positions: Vec<f32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut local_of = std::collections::HashMap::new();
    let mut part_of: Vec<u32> = Vec::new();
    for &t in &patch {
        for &v in &part.indices[3 * t as usize..3 * t as usize + 3] {
            let id = *local_of.entry(v).or_insert_with(|| {
                positions.extend_from_slice(&part.positions[3 * v as usize..3 * v as usize + 3]);
                part_of.push(v);
                part_of.len() as u32 - 1
            });
            indices.push(id);
        }
    }
    let patch_vertices = part_of.len() as u32;
    let patch_triangles = patch.len();
    for tool in tools {
        let base = (positions.len() / 3) as u32;
        positions.extend_from_slice(&tool.positions);
        for t in tool.indices.as_chunks::<3>().0 {
            let t = t.map(|v| v + base);
            indices.extend(if keep == Keep::Difference {
                [t[0], t[2], t[1]]
            } else {
                t
            });
        }
    }
    // Welding numbers points in the order the triangles first use them: the patch's come first, as
    // above, so a local point below `patch_vertices` is the part's point `part_of[id]`.
    let local = weld_indexed(&positions, &indices).map_err(RepairError::Failed)?;
    let arr = match arrangement::build(&local, &mut control.sub(0.0, 0.7))? {
        Ok(arr) => arr,
        Err(why) => {
            return Ok(Err(format!(
                "the cut could not be worked out exactly ({why})"
            )));
        }
    };
    let tool_solid = Solid::new(
        &crate::ops::concat(&tools.iter().collect::<Vec<_>>()).map_err(RepairError::Failed)?,
    );
    control.check()?;
    // Which faces stay: the part's pieces outside the tools (inside, for an intersection), and the
    // tools' pieces inside the part.
    let point = |id: u32| arr.pt(id).p;
    let kept: Vec<bool> = (0..arr.faces.len())
        .map(|f| {
            let [a, b, c] = arr.faces[f].map(point);
            let centre = [0, 1, 2].map(|k| (a[k] + b[k] + c[k]) / 3.0);
            if (arr.source[f] as usize) < patch_triangles {
                tool_solid.inside(centre) == (keep == Keep::Intersection)
            } else {
                part_solid.inside(centre)
            }
        })
        .collect();
    control.check()?;
    // The new mesh: the part's points (all of them with `whole`), then the tools' points and the
    // points the cut made, as they are used.
    let mut out_positions: Vec<f32> = if whole {
        part.positions.clone()
    } else {
        Vec::new()
    };
    let mut out_indices: Vec<u32> = Vec::new();
    if whole {
        for (t, tri) in part.indices.as_chunks::<3>().0.iter().enumerate() {
            if !near[t] {
                out_indices.extend_from_slice(tri);
            }
        }
    }
    let first_new = out_indices.len() / 3;
    let mut id_of: Vec<u32> = vec![u32::MAX; arr.point_count()];
    let mut map = |id: u32, out_positions: &mut Vec<f32>| -> u32 {
        if whole && id < patch_vertices && (id as usize) < arr.vertex_count {
            return part_of[id as usize];
        }
        if id_of[id as usize] == u32::MAX {
            id_of[id as usize] = (out_positions.len() / 3) as u32;
            out_positions.extend(arr.position(id));
        }
        id_of[id as usize]
    };
    for (f, face) in arr.faces.iter().enumerate() {
        if !kept[f] {
            continue;
        }
        let t = face.map(|id| map(id, &mut out_positions));
        out_indices.extend(t);
    }
    let mut mesh = super::compact(&out_positions, out_indices);
    // New points close enough to round to the same position are one point; the slivers between them
    // go (only new triangles can be such slivers).
    if let Some(joined) = crate::mesh::join_coincident(&mesh) {
        let tris = joined.indices.as_chunks::<3>().0;
        let mut indices = Vec::with_capacity(joined.indices.len());
        for (k, t) in tris.iter().enumerate() {
            if k < first_new || (t[0] != t[1] && t[1] != t[2] && t[0] != t[2]) {
                indices.extend_from_slice(t);
            }
        }
        mesh = super::compact(&joined.positions, indices);
    }
    control.progress(1.0);
    Ok(Ok(Cut { mesh, first_new }))
}

/// Tries a cut up to three times, the tools turned a little about their axes each time, until its
/// result is closed, oriented and free of new crossings.
fn sound_cut(
    part: &Mesh,
    part_solid: &Solid,
    tools: impl Fn(f64) -> Vec<Mesh>,
    keep: Keep,
    whole: bool,
    control: &mut Control,
) -> RepairResult<Result<Mesh, String>> {
    let mut last = String::new();
    for attempt in 0..3 {
        let span = 1.0 / 3.0;
        let mut part_control = control.sub(attempt as f64 * span, (attempt + 1) as f64 * span);
        let turn = attempt as f64 * 0.37 * std::f64::consts::TAU / SEGMENTS as f64;
        let made = match cut(
            part,
            part_solid,
            &tools(turn),
            keep,
            whole,
            &mut part_control.sub(0.0, 0.7),
        )? {
            Ok(made) => made,
            Err(why) => {
                last = why;
                continue;
            }
        };
        let checked = super::shell::verify(
            &made.mesh,
            if whole { made.first_new } else { 0 },
            &mut part_control.sub(0.7, 1.0),
        )?;
        if checked.closed && checked.oriented && checked.new_crossings == 0 {
            return Ok(Ok(made.mesh));
        }
        last = [
            (!checked.closed).then_some("not closed"),
            (!checked.oriented).then_some("not oriented"),
            (checked.new_crossings > 0).then_some("with crossing triangles"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" and ");
    }
    Ok(Err(last))
}

// ---------------------------------------------------------------------------------------------
// Apply

/// What Apply made.
#[derive(Debug, Default)]
pub struct Applied {
    /// The hollow part with its holes.
    pub mesh: Mesh,
    /// One per hole cut, when plugs were asked for.
    pub plugs: Vec<Mesh>,
    /// Holes left out, by index, with why.
    pub skipped: Vec<(usize, String)>,
    pub warnings: Vec<String>,
}

/// The cavity for the settings (see `shell::cavity`).
pub(crate) fn cavity(
    part: &Mesh,
    settings: &HollowSettings,
    control: &mut Control,
) -> RepairResult<super::shell::Cavity> {
    if let Some(problem) = settings.problem() {
        return Err(RepairError::Failed(problem));
    }
    super::shell::cavity(
        part,
        settings.thickness,
        settings.accuracy,
        settings.smooth_pairs(),
        control,
    )
}

/// The hollow part: `part` with `cavity` (as `cavity` made it) inside, the holes that check out cut
/// through its wall, and with `plugs` (a clearance, mm) a plug for each hole cut.
pub fn apply(
    part: &Mesh,
    cavity: &Mesh,
    settings: &HollowSettings,
    holes: &[Hole],
    plugs: Option<f64>,
    control: &mut Control,
) -> RepairResult<Applied> {
    let scene = Scene::new(part, Some(cavity), *settings);
    let mut applied = Applied::default();
    let mut good: Vec<(Hole, f64, f64)> = Vec::new();
    for i in 0..holes.len() {
        let check = check_hole(&scene, holes, i);
        match (check.ok, check.depth, check.room) {
            (true, Some(depth), Some(room)) => good.push((holes[i], depth, room)),
            _ => applied.skipped.push((
                i,
                check.reason.unwrap_or_else(|| "It was not checked.".into()),
            )),
        }
    }
    control.check()?;
    control.stage("Hollowing the part");
    let hollow = super::concat(&[part, &super::shell::turned(cavity.clone())])
        .map_err(|reason| RepairError::Failed(format!("Could not hollow the part: {reason}.")))?;
    if good.is_empty() {
        applied.mesh = hollow;
        control.progress(1.0);
        return Ok(applied);
    }
    let solid = Solid::new(&hollow);
    control.stage("Cutting the drain holes");
    let share = if plugs.is_some() { 0.6 } else { 1.0 };
    let tools = |turn: f64| -> Vec<Mesh> {
        good.iter()
            .map(|(h, depth, room)| hole_solid(h, *depth, *room, 0.0, turn))
            .collect()
    };
    let cut_mesh = match sound_cut(
        &hollow,
        &solid,
        tools,
        Keep::Difference,
        true,
        &mut control.sub(0.0, share),
    )? {
        Ok(mesh) => mesh,
        Err(why) => {
            return Err(RepairError::Failed(format!(
                "The drain holes could not be cut cleanly (the result was {why}). Move them a \
                 little or change their size."
            )));
        }
    };
    applied.mesh = cut_mesh;
    if let Some(clearance) = plugs {
        control.stage("Making the plugs");
        let n = good.len().max(1) as f64;
        for (k, (h, depth, room)) in good.iter().enumerate() {
            let span = (1.0 - share) / n;
            let mut part_control =
                control.sub(share + k as f64 * span, share + (k + 1) as f64 * span);
            let made = sound_cut(
                &hollow,
                &solid,
                |turn| vec![hole_solid(h, *depth, *room, clearance, turn)],
                Keep::Intersection,
                false,
                &mut part_control,
            )?;
            match made {
                Ok(plug) if plug.triangle_count() > 0 => applied.plugs.push(plug),
                Ok(_) => applied.warnings.push(format!(
                    "Hole {} is too narrow for a plug with a clearance of {} mm, so it has none.",
                    k + 1,
                    round1(clearance)
                )),
                Err(why) => applied.warnings.push(format!(
                    "No plug was made for hole {}: it came out {why}.",
                    k + 1
                )),
            }
        }
    }
    control.progress(1.0);
    Ok(applied)
}

#[cfg(test)]
mod tests;
