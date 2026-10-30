//! Packing: parts arranged on the platform in as little room as possible.
//!
//! - **3D Bounding box** places each part's box, unturned, at the lowest free corner left by the
//!   boxes before it (stacked, or in one layer with "only in 2D").
//! - **2D Scanline** lays the parts out in one layer on the platform by their real outline,
//!   turned about Z in steps, each at the first free place scanning row by row.
//! - **3D Scanline** drops each part, turned about Z in steps and optionally upside down, wherever
//!   its top ends lowest, onto what is already there.
//! - **3D Monte Carlo** repeats a seeded random shake: parts in order of priority, each turned as
//!   it may be, settle where their top ends lowest. The run with the lowest packing height wins.
//!
//! The shape-aware methods see a part as columns on a grid of `voxel` cells: in each column, from
//! the lowest to the highest point of its surface there. That covers the solid between, so packed
//! parts never overlap. Each later part comes down from above onto those columns and never goes
//! under another, so every part can be lifted straight out: nothing can interlock.
//!
//! Parts that do not fit go beside the platform, past its +X side, and are listed. Locked parts and
//! the parts not being packed stay where they are; the others pack round them.

use rayon::prelude::*;
use serde::Deserialize;

use super::orient::{BoxAxes, Turn, min_box};
use super::{Aabb, IDENTITY_ROT, Rot, V3, World, add, apply, compose, rot_z, size, sub};
use crate::repair::{Control, RepairResult};

/// How a part may be turned to pack better (Monte Carlo); `Default` takes the method's setting.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Rotation {
    Default,
    None,
    Z,
    Free,
}

impl Rotation {
    /// Its name in the window and in a project file.
    pub fn word(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::None => "none",
            Self::Z => "z",
            Self::Free => "free",
        }
    }

    pub fn from_word(word: &str) -> Option<Self> {
        [Self::Default, Self::None, Self::Z, Self::Free]
            .into_iter()
            .find(|r| r.word() == word)
    }
}

/// A packing method and its settings (mm; `performance` 1, accurate, to 9, fast).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "method", rename_all = "kebab-case")]
pub enum Method {
    BoundingBox {
        distance: f64,
        voxel: f64,
        wall: f64,
        floor: f64,
        ceiling: f64,
        stack: bool,
    },
    Scanline2d {
        performance: u32,
        align: bool,
        #[serde(rename = "zSteps")]
        z_steps: u32,
        wall: f64,
        /// Cells added round each part's outline: 0, 1 or 2.
        spacing: u32,
        voxel: f64,
    },
    Scanline3d {
        performance: u32,
        voxel: f64,
        #[serde(rename = "zSteps")]
        z_steps: u32,
        flip: bool,
        wall: f64,
        floor: f64,
        ceiling: f64,
    },
    MonteCarlo {
        performance: u32,
        distance: f64,
        rotation: Rotation,
        wall: f64,
        floor: f64,
        ceiling: f64,
        seed: u64,
    },
}

/// A part to pack: where it is now, its packing priority (higher first) and permitted rotation.
pub struct Item {
    pub world: World,
    pub priority: i32,
    pub rotation: Rotation,
}

/// Where each item went (in input order), whether it fits on the platform, and how high the
/// packed parts reach.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub turns: Vec<Turn>,
    pub fits: Vec<bool>,
    pub height: f64,
}

/// The smallest gap left between parts stacked on each other, so no two ever touch.
const CLEARANCE: f64 = 0.05;
/// Room between the parts set beside the platform.
const ASIDE_GAP: f64 = 10.0;

fn check_settings(method: &Method, platform: V3) -> Result<(), String> {
    let finite = |v: f64, min: f64| v.is_finite() && v >= min;
    let ok = match *method {
        Method::BoundingBox {
            distance,
            voxel,
            wall,
            floor,
            ceiling,
            ..
        } => {
            finite(distance, 0.0)
                && finite(voxel, 0.001)
                && finite(wall, 0.0)
                && finite(floor, 0.0)
                && finite(ceiling, 0.0)
        }
        Method::Scanline2d {
            performance,
            z_steps,
            wall,
            spacing,
            voxel,
            ..
        } => {
            (1..=9).contains(&performance)
                && (1..=18).contains(&z_steps)
                && finite(wall, 0.0)
                && spacing <= 2
                && finite(voxel, 0.1)
        }
        Method::Scanline3d {
            performance,
            voxel,
            z_steps,
            wall,
            floor,
            ceiling,
            ..
        } => {
            (1..=9).contains(&performance)
                && finite(voxel, 0.1)
                && (1..=18).contains(&z_steps)
                && finite(wall, 0.0)
                && finite(floor, 0.0)
                && finite(ceiling, 0.0)
        }
        Method::MonteCarlo {
            performance,
            distance,
            wall,
            floor,
            ceiling,
            ..
        } => {
            (1..=9).contains(&performance)
                && finite(distance, 0.0)
                && finite(wall, 0.0)
                && finite(floor, 0.0)
                && finite(ceiling, 0.0)
        }
    };
    if !ok {
        return Err("Could not pack the parts: a packing setting is out of range.".into());
    }
    // A grid of more than about 16 million cells would take too long and too much memory.
    let voxel = match *method {
        Method::Scanline2d { voxel, .. } | Method::Scanline3d { voxel, .. } => voxel,
        Method::MonteCarlo { performance, .. } => mc_voxel(performance),
        Method::BoundingBox { .. } => return Ok(()),
    };
    if (platform[0] / voxel) * (platform[1] / voxel) > 16e6 {
        return Err(format!(
            "A voxel size of {voxel} mm is too fine for a platform this big. Enter a larger one."
        ));
    }
    Ok(())
}

/// Packs `items` on a `platform` (width, depth, height from the origin) round `obstacles`.
pub fn pack(
    items: &[Item],
    obstacles: &[World],
    platform: V3,
    method: &Method,
    control: &mut Control,
) -> RepairResult<Result<Outcome, String>> {
    if let Err(e) = check_settings(method, platform) {
        return Ok(Err(e));
    }
    let mut order: Vec<usize> = (0..items.len())
        .filter(|&i| items[i].world.bounds().is_some())
        .collect();
    let volume = |i: usize| {
        let s = size(&items[i].world.bounds().unwrap_or_default());
        s[0] * s[1] * s[2]
    };
    order.sort_by(|&a, &b| {
        items[b]
            .priority
            .cmp(&items[a].priority)
            .then(volume(b).total_cmp(&volume(a)))
            .then(a.cmp(&b))
    });
    let placed = match *method {
        Method::BoundingBox {
            distance,
            voxel,
            wall,
            floor,
            ceiling,
            stack,
        } => pack_boxes(
            items,
            &order,
            obstacles,
            platform,
            Boxes {
                // Parts that touch face to face collide (their faces overlap), so never less.
                distance: distance.max(CLEARANCE),
                voxel,
                wall,
                floor,
                top: platform[2] - ceiling,
                stack,
            },
            control,
        )?,
        Method::Scanline2d {
            performance,
            align,
            z_steps,
            wall,
            spacing,
            voxel,
        } => {
            let field = Field::new(platform, voxel, wall, spacing as usize, obstacles);
            let shapes = |i: usize| {
                let base = if align {
                    min_box(&items[i].world, BoxAxes::Z).map_or(IDENTITY_ROT, |t| t.rotation)
                } else {
                    IDENTITY_ROT
                };
                z_turns(z_steps)
                    .map(move |r| compose(&r, &base))
                    .collect::<Vec<_>>()
            };
            pack_columns(
                items,
                &order,
                field,
                Placing {
                    mode: Mode::Flat,
                    stride: stride(performance),
                    grow: spacing as usize,
                },
                &shapes,
                control,
            )?
        }
        Method::Scanline3d {
            performance,
            voxel,
            z_steps,
            flip,
            wall,
            floor,
            ceiling,
        } => {
            let field = Field::new(platform, voxel, wall, 0, obstacles);
            let mode = Mode::Drop {
                floor,
                top: platform[2] - ceiling,
                gap: CLEARANCE,
            };
            let shapes = |_: usize| {
                let mut turns: Vec<Rot> = z_turns(z_steps).collect();
                if flip {
                    let over = rot_x_half();
                    turns.extend(z_turns(z_steps).map(|r| compose(&r, &over)));
                }
                turns
            };
            pack_columns(
                items,
                &order,
                field,
                Placing {
                    mode,
                    stride: stride(performance),
                    grow: 0,
                },
                &shapes,
                control,
            )?
        }
        Method::MonteCarlo {
            performance,
            distance,
            rotation,
            wall,
            floor,
            ceiling,
            seed,
        } => monte_carlo(
            items,
            &order,
            obstacles,
            platform,
            MonteCarlo {
                performance,
                distance,
                rotation,
                wall,
                floor,
                top: platform[2] - ceiling,
                seed,
            },
            control,
        )?,
    };
    Ok(Ok(finish(items, placed, platform)))
}

/// Every item's turn: its packed place, or for the ones that did not fit a place beside the
/// platform, past its +X side, in a row along X.
fn finish(items: &[Item], placed: Vec<Option<Turn>>, platform: V3) -> Outcome {
    let mut x = platform[0] + 2.0 * ASIDE_GAP;
    let mut height: f64 = 0.0;
    let mut turns = Vec::with_capacity(items.len());
    let mut fits = Vec::with_capacity(items.len());
    for (item, place) in items.iter().zip(placed) {
        let b = item.world.bounds().unwrap_or_default();
        match place {
            Some(turn) => {
                height = height.max(super::orient::moved_top(&item.world, &turn));
                turns.push(turn);
                fits.push(true);
            }
            None => {
                turns.push(Turn {
                    rotation: IDENTITY_ROT,
                    pivot: super::centre(&b),
                    shift: [x - b[0][0], -b[0][1], -b[0][2]],
                });
                x += size(&b)[0] + ASIDE_GAP;
                fits.push(false);
            }
        }
    }
    Outcome {
        turns,
        fits,
        height,
    }
}

fn rot_x_half() -> Rot {
    [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]]
}

/// `n` turns about Z, evenly spaced over a full turn, the first none.
fn z_turns(n: u32) -> impl Iterator<Item = Rot> {
    let n = n.max(1);
    (0..n).map(move |k| rot_z(std::f64::consts::TAU * f64::from(k) / f64::from(n)))
}

/// Cells skipped between the places tried: 1 (every place) when accurate, up to 3 when fast.
fn stride(performance: u32) -> usize {
    1 + (performance.clamp(1, 9) as usize - 1) / 3
}

// ---------------------------------------------------------------------------------------------
// Bounding boxes

struct Boxes {
    distance: f64,
    voxel: f64,
    wall: f64,
    floor: f64,
    top: f64,
    stack: bool,
}

fn boxes_overlap(a: &Aabb, b: &Aabb, gap: f64) -> bool {
    (0..3).all(|k| a[0][k] < b[1][k] + gap - 1e-9 && b[0][k] < a[1][k] + gap - 1e-9)
}

fn pack_boxes(
    items: &[Item],
    order: &[usize],
    obstacles: &[World],
    platform: V3,
    s: Boxes,
    control: &mut Control,
) -> RepairResult<Vec<Option<Turn>>> {
    let lo = [s.wall, s.wall, s.floor];
    let hi = [platform[0] - s.wall, platform[1] - s.wall, s.top];
    let mut taken: Vec<Aabb> = obstacles.iter().filter_map(World::bounds).collect();
    // The corners new boxes may go to: the platform's own, and beside, behind and on top of every
    // box placed so far.
    let mut corners: Vec<V3> = vec![lo];
    for b in &taken {
        corners.push([b[1][0] + s.distance, lo[1], lo[2]]);
        corners.push([lo[0], b[1][1] + s.distance, lo[2]]);
        corners.push([b[1][0] + s.distance, b[0][1], lo[2]]);
        corners.push([b[0][0], b[1][1] + s.distance, lo[2]]);
        if s.stack {
            corners.push([b[0][0], b[0][1], b[1][2] + s.distance]);
        }
    }
    let mut placed = vec![None; items.len()];
    for (n, &i) in order.iter().enumerate() {
        control.check()?;
        let b = items[i].world.bounds().unwrap_or_default();
        let sz = size(&b);
        let fits_at = |p: V3| {
            let cand = [p, add(p, sz)];
            (0..3).all(|k| cand[0][k] >= lo[k] - 1e-9 && cand[1][k] <= hi[k] + 1e-9)
                && (s.stack || p[2] <= lo[2] + 1e-9)
                && !taken.iter().any(|t| boxes_overlap(&cand, t, s.distance))
        };
        corners.sort_by(|a, b| {
            a[2].total_cmp(&b[2])
                .then(a[1].total_cmp(&b[1]))
                .then(a[0].total_cmp(&b[0]))
        });
        let mut spot = corners.iter().copied().find(|&p| fits_at(p));
        if spot.is_none() {
            // No corner has room: scan the free room in voxel steps, lowest first.
            let steps = |k: usize| ((hi[k] - lo[k] - sz[k]) / s.voxel).floor().max(-1.0) as i64;
            let (nx, ny, nz) = (steps(0), steps(1), if s.stack { steps(2) } else { 0 });
            if nx >= 0 && ny >= 0 && nz >= 0 && (nx + 1) * (ny + 1) * (nz + 1) <= 4_000_000 {
                'scan: for z in 0..=nz {
                    for y in 0..=ny {
                        for x in 0..=nx {
                            let p = [
                                lo[0] + x as f64 * s.voxel,
                                lo[1] + y as f64 * s.voxel,
                                lo[2] + z as f64 * s.voxel,
                            ];
                            if fits_at(p) {
                                spot = Some(p);
                                break 'scan;
                            }
                        }
                    }
                }
            }
        }
        if let Some(p) = spot {
            let cand = [p, add(p, sz)];
            taken.push(cand);
            corners.push([cand[1][0] + s.distance, p[1], p[2]]);
            corners.push([p[0], cand[1][1] + s.distance, p[2]]);
            if s.stack {
                corners.push([p[0], p[1], cand[1][2] + s.distance]);
            }
            placed[i] = Some(Turn {
                rotation: IDENTITY_ROT,
                pivot: super::centre(&b),
                shift: sub(p, b[0]),
            });
        }
        control.progress((n + 1) as f64 / order.len() as f64);
    }
    Ok(placed)
}

// ---------------------------------------------------------------------------------------------
// Columns

/// A part turned one way, as columns: for each cell of its grid, the lowest and highest point of
/// its surface there, from the bottom of the turned part. Empty cells have lo = +inf, hi = -inf.
struct Shape {
    rot: Rot,
    pivot: V3,
    /// The turned part's box corner the grid starts at.
    min: V3,
    nx: usize,
    ny: usize,
    lo: Vec<f64>,
    hi: Vec<f64>,
    cells: Vec<u32>,
    height: f64,
}

impl Shape {
    /// The part turned by `rot` about its box centre, on cells of `voxel`, grown by `grow` cells
    /// round its outline (the shape's grid gains that border).
    fn new(world: &World, rot: &Rot, voxel: f64, grow: usize) -> Option<Shape> {
        let pivot = super::centre(&world.bounds()?);
        let points: Vec<V3> = world
            .points
            .iter()
            .map(|&p| add(apply(rot, sub(p, pivot)), pivot))
            .collect();
        let b = super::bounds(&points)?;
        let g = grow as f64 * voxel;
        let min = [b[0][0] - g, b[0][1] - g, b[0][2]];
        let nx = ((b[1][0] + g - min[0]) / voxel).floor() as usize + 1;
        let ny = ((b[1][1] + g - min[1]) / voxel).floor() as usize + 1;
        if nx.saturating_mul(ny) > 4_000_000 {
            return None;
        }
        let mut lo = vec![f64::INFINITY; nx * ny];
        let mut hi = vec![f64::NEG_INFINITY; nx * ny];
        let mut mark = |a: V3, b: V3, c: V3| {
            let (x0, x1) = (a[0].min(b[0]).min(c[0]), a[0].max(b[0]).max(c[0]));
            let (y0, y1) = (a[1].min(b[1]).min(c[1]), a[1].max(b[1]).max(c[1]));
            let (z0, z1) = (
                a[2].min(b[2]).min(c[2]) - min[2],
                a[2].max(b[2]).max(c[2]) - min[2],
            );
            let cell =
                |v: f64, o: f64, n: usize| (((v - o) / voxel).floor().max(0.0) as usize).min(n - 1);
            for j in cell(y0, min[1], ny)..=cell(y1, min[1], ny) {
                for i in cell(x0, min[0], nx)..=cell(x1, min[0], nx) {
                    let k = j * nx + i;
                    lo[k] = lo[k].min(z0);
                    hi[k] = hi[k].max(z1);
                }
            }
        };
        for &t in &world.triangles {
            let [a, b, c] = t.map(|i| points[i as usize]);
            split(a, b, c, voxel, 0, &mut mark);
        }
        if grow > 0 {
            spread(&mut lo, nx, ny, grow, f64::min);
            spread(&mut hi, nx, ny, grow, f64::max);
        }
        let cells = (0..nx * ny)
            .filter(|&k| lo[k] <= hi[k])
            .map(|k| k as u32)
            .collect();
        Some(Shape {
            rot: *rot,
            pivot,
            min,
            nx,
            ny,
            lo,
            hi,
            cells,
            height: b[1][2] - b[0][2],
        })
    }

    /// The turn that puts the grid's corner at `corner` (world mm).
    fn turn_to(&self, corner: V3) -> Turn {
        Turn {
            rotation: self.rot,
            pivot: self.pivot,
            shift: sub(corner, self.min),
        }
    }
}

/// Splits a triangle until it is no wider than a cell, handing each piece to `mark`.
fn split(a: V3, b: V3, c: V3, voxel: f64, depth: u32, mark: &mut impl FnMut(V3, V3, V3)) {
    let wide = |k: usize| a[k].max(b[k]).max(c[k]) - a[k].min(b[k]).min(c[k]);
    if depth >= 16 || (wide(0) <= voxel && wide(1) <= voxel) {
        mark(a, b, c);
        return;
    }
    let d2 = |p: V3, q: V3| (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2);
    let mid = |p: V3, q: V3| {
        [
            (p[0] + q[0]) / 2.0,
            (p[1] + q[1]) / 2.0,
            (p[2] + q[2]) / 2.0,
        ]
    };
    let (ab, bc, ca) = (d2(a, b), d2(b, c), d2(c, a));
    if ab >= bc && ab >= ca {
        let m = mid(a, b);
        split(a, m, c, voxel, depth + 1, mark);
        split(m, b, c, voxel, depth + 1, mark);
    } else if bc >= ca {
        let m = mid(b, c);
        split(a, b, m, voxel, depth + 1, mark);
        split(a, m, c, voxel, depth + 1, mark);
    } else {
        let m = mid(c, a);
        split(a, b, m, voxel, depth + 1, mark);
        split(m, b, c, voxel, depth + 1, mark);
    }
}

/// Each cell takes `pick` of the cells within `r` of it (a square), so a shape grows by `r` cells.
fn spread(values: &mut [f64], nx: usize, ny: usize, r: usize, pick: fn(f64, f64) -> f64) {
    let src = values.to_vec();
    let mut rows = src.clone();
    for j in 0..ny {
        for i in 0..nx {
            let (a, b) = (i.saturating_sub(r), (i + r).min(nx - 1));
            rows[j * nx + i] = (a..=b)
                .map(|x| src[j * nx + x])
                .reduce(pick)
                .unwrap_or(src[j * nx + i]);
        }
    }
    for j in 0..ny {
        for i in 0..nx {
            let (a, b) = (j.saturating_sub(r), (j + r).min(ny - 1));
            values[j * nx + i] = (a..=b)
                .map(|y| rows[y * nx + i])
                .reduce(pick)
                .unwrap_or(rows[j * nx + i]);
        }
    }
}

/// The platform as columns: for each cell, the top of what stands there (-inf: empty; +inf: not
/// usable, outside the margin).
#[derive(Clone)]
struct Field {
    voxel: f64,
    nx: usize,
    ny: usize,
    top: Vec<f64>,
}

impl Field {
    /// The platform's cells, those within `wall` of its sides blocked, with the obstacles' columns
    /// on it (grown by `grow` cells).
    fn new(platform: V3, voxel: f64, wall: f64, grow: usize, obstacles: &[World]) -> Field {
        let nx = (platform[0] / voxel).floor().max(0.0) as usize;
        let ny = (platform[1] / voxel).floor().max(0.0) as usize;
        let mut top = vec![f64::NEG_INFINITY; nx * ny];
        let inside = |i: usize, extent: f64| {
            i as f64 * voxel >= wall - 1e-9 && (i + 1) as f64 * voxel <= extent - wall + 1e-9
        };
        for j in 0..ny {
            for i in 0..nx {
                if !(inside(i, platform[0]) && inside(j, platform[1])) {
                    top[j * nx + i] = f64::INFINITY;
                }
            }
        }
        let mut field = Field { voxel, nx, ny, top };
        for world in obstacles {
            if let Some(shape) = Shape::new(world, &IDENTITY_ROT, voxel, grow) {
                field.stamp_at_world(&shape);
            }
        }
        field
    }

    /// Marks an obstacle's columns where it stands, clipped to the platform.
    fn stamp_at_world(&mut self, shape: &Shape) {
        let v = self.voxel;
        for &k in &shape.cells {
            let (ci, cj) = (k as usize % shape.nx, k as usize / shape.nx);
            // The cells the shape's cell overlaps: it starts at shape.min, not on the grid.
            let x0 = shape.min[0] + ci as f64 * v;
            let y0 = shape.min[1] + cj as f64 * v;
            for (fi, fj) in [
                ((x0 / v).floor(), (y0 / v).floor()),
                ((x0 / v).floor() + 1.0, (y0 / v).floor()),
                ((x0 / v).floor(), (y0 / v).floor() + 1.0),
                ((x0 / v).floor() + 1.0, (y0 / v).floor() + 1.0),
            ] {
                if fi < 0.0 || fj < 0.0 || fi >= self.nx as f64 || fj >= self.ny as f64 {
                    continue;
                }
                let f = fj as usize * self.nx + fi as usize;
                if self.top[f] != f64::INFINITY {
                    self.top[f] = self.top[f].max(shape.min[2] + shape.hi[k as usize]);
                }
            }
        }
    }

    /// Marks a packed part's columns, its grid corner at cell (`ix`, `iy`) and height `z`.
    fn stamp(&mut self, shape: &Shape, ix: usize, iy: usize, z: f64) {
        for &k in &shape.cells {
            let (ci, cj) = (k as usize % shape.nx, k as usize / shape.nx);
            let f = (iy + cj) * self.nx + ix + ci;
            self.top[f] = self.top[f].max(z + shape.hi[k as usize]);
        }
    }
}

/// How the shape-aware methods put a part down.
#[derive(Clone, Copy)]
enum Mode {
    /// One layer: only on empty cells, resting on the platform.
    Flat,
    /// Dropped from above onto what is there, `gap` clear of it, at least `floor` up, its top no
    /// higher than `top`.
    Drop { floor: f64, top: f64, gap: f64 },
}

/// How `pack_columns` puts parts down: the mode, every `stride`-th cell tried, and the cells each
/// part's outline grows by.
#[derive(Clone, Copy)]
struct Placing {
    mode: Mode,
    stride: usize,
    grow: usize,
}

/// Where a shape goes best: its grid corner's cell and height, and the score (lower is better).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Spot {
    score: (f64, f64),
    iy: usize,
    ix: usize,
    z: f64,
}

fn better(a: &Spot, b: &Spot) -> bool {
    (a.score.0, a.score.1, a.iy, a.ix) < (b.score.0, b.score.1, b.iy, b.ix)
}

/// The best place for `shape` on `field`, trying every `stride`-th cell. Flat takes the first
/// free place row by row; Drop the place where the part's top ends lowest. Cancel is looked at
/// between rows.
fn best_spot(
    field: &Field,
    shape: &Shape,
    mode: Mode,
    stride: usize,
    cancel: &std::sync::atomic::AtomicBool,
) -> Option<Spot> {
    if shape.nx > field.nx || shape.ny > field.ny {
        return None;
    }
    let rows: Vec<usize> = (0..=field.ny - shape.ny).step_by(stride).collect();
    let row_best = |&iy: &usize| -> Option<Spot> {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        let mut best: Option<Spot> = None;
        for ix in (0..=field.nx - shape.nx).step_by(stride) {
            let spot = match mode {
                Mode::Flat => {
                    let free = shape.cells.iter().all(|&k| {
                        let (ci, cj) = (k as usize % shape.nx, k as usize / shape.nx);
                        field.top[(iy + cj) * field.nx + ix + ci] == f64::NEG_INFINITY
                    });
                    if !free {
                        continue;
                    }
                    // The first free place in this row is this row's best.
                    Spot {
                        score: (iy as f64 + shape.ny as f64, 0.0),
                        iy,
                        ix,
                        z: 0.0,
                    }
                }
                Mode::Drop { floor, top, gap } => {
                    let bound = best.map_or(top - shape.height, |b| b.score.0 - shape.height);
                    let mut z = floor;
                    let mut blocked = false;
                    for &k in &shape.cells {
                        let (ci, cj) = (k as usize % shape.nx, k as usize / shape.nx);
                        let under = field.top[(iy + cj) * field.nx + ix + ci];
                        if under == f64::INFINITY {
                            blocked = true;
                            break;
                        }
                        if under > f64::NEG_INFINITY {
                            z = z.max(under + gap - shape.lo[k as usize]);
                            if z > bound + 1e-9 {
                                blocked = true;
                                break;
                            }
                        }
                    }
                    if blocked {
                        continue;
                    }
                    Spot {
                        score: (z + shape.height, z),
                        iy,
                        ix,
                        z,
                    }
                }
            };
            if best.is_none_or(|b| better(&spot, &b)) {
                best = Some(spot);
            }
            if matches!(mode, Mode::Flat) {
                break;
            }
        }
        best
    };
    let found: Vec<Spot> = rows.par_iter().filter_map(row_best).collect();
    found
        .into_iter()
        .reduce(|a, b| if better(&b, &a) { b } else { a })
}

/// Places each item, in `order`, at its best spot over the turns `turns(i)` offers, its outline
/// grown by `grow` cells. Returns each item's turn, None for those that do not fit.
fn pack_columns(
    items: &[Item],
    order: &[usize],
    mut field: Field,
    placing: Placing,
    turns: &dyn Fn(usize) -> Vec<Rot>,
    control: &mut Control,
) -> RepairResult<Vec<Option<Turn>>> {
    let Placing { mode, stride, grow } = placing;
    let mut placed = vec![None; items.len()];
    for (n, &i) in order.iter().enumerate() {
        control.check()?;
        let cancel = control.cancel_flag();
        let shapes: Vec<Shape> = turns(i)
            .par_iter()
            .filter_map(|r| Shape::new(&items[i].world, r, field.voxel, grow))
            .collect();
        control.check()?;
        let best = shapes
            .iter()
            .enumerate()
            .filter_map(|(s, shape)| {
                best_spot(&field, shape, mode, stride, cancel).map(|spot| (s, spot))
            })
            .reduce(|a, b| if better(&b.1, &a.1) { b } else { a });
        control.check()?;
        if let Some((s, spot)) = best {
            let shape = &shapes[s];
            let z = match mode {
                Mode::Flat => 0.0,
                Mode::Drop { .. } => spot.z,
            };
            field.stamp(shape, spot.ix, spot.iy, z);
            let v = field.voxel;
            placed[i] = Some(shape.turn_to([spot.ix as f64 * v, spot.iy as f64 * v, z]));
        }
        control.progress((n + 1) as f64 / order.len() as f64);
    }
    Ok(placed)
}

// ---------------------------------------------------------------------------------------------
// Monte Carlo

struct MonteCarlo {
    performance: u32,
    distance: f64,
    rotation: Rotation,
    wall: f64,
    floor: f64,
    top: f64,
    seed: u64,
}

/// The cell size Monte Carlo works on: 1 mm when accurate, 4 mm when fast.
fn mc_voxel(performance: u32) -> f64 {
    1.0 + f64::from(performance.clamp(1, 9) - 1) * 0.375
}

/// SplitMix64: a small, fast generator; the same seed gives the same numbers on every machine.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// The 24 ways a part can face along the axes.
fn axis_turns() -> Vec<Rot> {
    let quarter_x = [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]];
    let quarter_y = [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]];
    let mut ups = vec![IDENTITY_ROT, rot_x_half(), quarter_x];
    ups.push(compose(&rot_x_half(), &quarter_x));
    ups.push(quarter_y);
    ups.push(compose(&rot_x_half(), &quarter_y));
    let mut out = Vec::new();
    for up in ups {
        for k in 0..4 {
            out.push(compose(
                &rot_z(std::f64::consts::FRAC_PI_2 * f64::from(k)),
                &up,
            ));
        }
    }
    out
}

fn monte_carlo(
    items: &[Item],
    order: &[usize],
    obstacles: &[World],
    platform: V3,
    s: MonteCarlo,
    control: &mut Control,
) -> RepairResult<Vec<Option<Turn>>> {
    let voxel = mc_voxel(s.performance);
    // Both parts of a pair grow, so each grows by half the distance.
    let grow = (s.distance / (2.0 * voxel)).ceil() as usize;
    let field = Field::new(platform, voxel, s.wall, grow, obstacles);
    let mode = Mode::Drop {
        floor: s.floor,
        top: s.top,
        gap: s.distance.max(CLEARANCE),
    };
    let rounds = 12 - s.performance.clamp(1, 9) as usize;
    let tries = 4;
    let z_steps = 12;
    let axes = axis_turns();
    let mut random = Random(s.seed);
    let mut best: Option<(usize, f64, Vec<Option<Turn>>)> = None;
    for round in 0..rounds {
        // The first round packs in priority and size order; the others shake parts of the same
        // priority into a random order.
        let mut sequence = order.to_vec();
        if round > 0 {
            let mut start = 0;
            while start < sequence.len() {
                let p = items[sequence[start]].priority;
                let end = start
                    + sequence[start..]
                        .iter()
                        .take_while(|&&i| items[i].priority == p)
                        .count();
                for k in (start + 1..end).rev() {
                    let j = start + random.below(k - start + 1);
                    sequence.swap(k, j);
                }
                start = end;
            }
        }
        // Each part's turns for this round, drawn before packing so the result does not depend on
        // the order threads finish in.
        let turns: Vec<Vec<Rot>> = items
            .iter()
            .map(|item| {
                let rotation = if item.rotation == Rotation::Default {
                    s.rotation
                } else {
                    item.rotation
                };
                let mut list = vec![IDENTITY_ROT];
                for _ in 0..tries {
                    match rotation {
                        Rotation::None | Rotation::Default => {}
                        Rotation::Z => list.push(rot_z(
                            std::f64::consts::TAU * random.below(z_steps) as f64 / z_steps as f64,
                        )),
                        Rotation::Free => {
                            let up = axes[random.below(axes.len())];
                            let z = rot_z(
                                std::f64::consts::TAU * random.below(z_steps) as f64
                                    / z_steps as f64,
                            );
                            list.push(compose(&z, &up));
                        }
                    }
                }
                list.dedup();
                list
            })
            .collect();
        let start = round as f64 / rounds as f64;
        let mut sub = control.sub(start, start + 1.0 / rounds as f64);
        let placed = pack_columns(
            items,
            &sequence,
            field.clone(),
            Placing {
                mode,
                stride: 1,
                grow,
            },
            &|i| turns[i].clone(),
            &mut sub,
        )?;
        let missing = placed
            .iter()
            .zip(items)
            .filter(|(p, it)| p.is_none() && it.world.bounds().is_some())
            .count();
        let height = items
            .iter()
            .zip(&placed)
            .filter_map(|(it, p)| p.as_ref().map(|t| super::orient::moved_top(&it.world, t)))
            .fold(0.0, f64::max);
        if best
            .as_ref()
            .is_none_or(|b| (missing, height) < (b.0, b.1 - 1e-9))
        {
            best = Some((missing, height, placed));
        }
    }
    Ok(best.map(|b| b.2).unwrap_or_else(|| vec![None; items.len()]))
}
