//! Upskins and downskins: triangles facing up (down) whose surface is tilted against the X/Y
//! plane by at most a threshold angle, gathered into components of triangles that share edges.
//! Components no larger than the minimum size are ignored; "filter small triangles" adds small
//! regions at safe angles that a skin of one kind surrounds.

use serde::{Deserialize, Serialize};

use super::tilt_degrees;
use crate::arrange::World;
use crate::mesh::Mesh;
use crate::repair::topology::Edges;
use crate::repair::{Control, RepairResult};

/// The colour class of a triangle in `classes`.
pub const PART: u8 = 0;
pub const UPSKIN: u8 = 1;
pub const DOWNSKIN: u8 = 2;

/// `SkinSettings` in src/analysis/types.ts. Angles in degrees, the size in mm².
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SkinSettings {
    pub upskins: bool,
    pub up_angle: f64,
    pub downskins: bool,
    pub down_angle: f64,
    pub min_component: f64,
    pub filter_small: bool,
}

/// Highest threshold angle the dialog takes.
pub const MAX_ANGLE: f64 = 89.9;

/// One kind's totals: `SkinTotals` in src/analysis/types.ts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkinTotals {
    pub analyzed: bool,
    pub angle: f64,
    /// mm².
    pub area: f64,
    pub components: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Skins {
    pub up: SkinTotals,
    pub down: SkinTotals,
    /// PART, UPSKIN or DOWNSKIN per triangle.
    pub classes: Vec<u8>,
}

fn root(parent: &mut [u32], mut v: u32) -> u32 {
    while parent[v as usize] != v {
        let up = parent[parent[v as usize] as usize];
        parent[v as usize] = up;
        v = up;
    }
    v
}

/// Joins triangles of the same class across shared edges; the root of every triangle.
fn regions(edges: &Edges, classes: &[u8]) -> Vec<u32> {
    let mut parent: Vec<u32> = (0..classes.len() as u32).collect();
    for e in 0..edges.len() as u32 {
        let sides = edges.sides(e);
        let Some(&first) = sides.first() else {
            continue;
        };
        let a = first / 3;
        for &s in &sides[1..] {
            let b = s / 3;
            if classes[a as usize] == classes[b as usize] {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                if ra != rb {
                    parent[ra.max(rb) as usize] = ra.min(rb);
                }
            }
        }
    }
    for t in 0..classes.len() as u32 {
        let r = root(&mut parent, t);
        parent[t as usize] = r;
    }
    parent
}

/// Classifies the triangles of the placed part (`world`, with the topology of `mesh`).
pub fn skins(
    mesh: &Mesh,
    world: &World,
    s: &SkinSettings,
    control: &mut Control,
) -> RepairResult<Skins> {
    let n = world.triangles.len();
    let mut classes = vec![PART; n];
    let mut areas = vec![0.0; n];
    for (i, &t) in world.triangles.iter().enumerate() {
        let (normal, area) = world.normal_area(t);
        areas[i] = area;
        if area <= 0.0 {
            continue;
        }
        let tilt = tilt_degrees(normal[2]);
        if s.upskins && normal[2] > 0.0 && tilt <= s.up_angle + 1e-9 {
            classes[i] = UPSKIN;
        } else if s.downskins && normal[2] < 0.0 && tilt <= s.down_angle + 1e-9 {
            classes[i] = DOWNSKIN;
        }
    }
    control.progress(0.3);
    control.check()?;
    let edges = Edges::new(mesh);
    control.progress(0.5);
    control.check()?;
    if s.filter_small {
        // Regions at safe angles, no larger than the minimum size, whose every neighbour is a skin
        // of one kind, become that skin.
        let roots = regions(&edges, &classes);
        let mut size = vec![0.0; n];
        for t in 0..n {
            if classes[t] == PART {
                size[roots[t] as usize] += areas[t];
            }
        }
        // Per region root: the one skin kind around it, or u8::MAX when it borders more.
        let mut around = vec![PART; n];
        for e in 0..edges.len() as u32 {
            let sides = edges.sides(e);
            for &a in sides {
                for &b in sides {
                    let (ta, tb) = ((a / 3) as usize, (b / 3) as usize);
                    if classes[ta] != PART || classes[tb] == PART {
                        continue;
                    }
                    let r = roots[ta] as usize;
                    around[r] = match around[r] {
                        PART => classes[tb],
                        k if k == classes[tb] => k,
                        _ => u8::MAX,
                    };
                }
            }
        }
        for t in 0..n {
            let r = roots[t] as usize;
            if classes[t] == PART
                && size[r] <= s.min_component
                && matches!(around[r], UPSKIN | DOWNSKIN)
            {
                classes[t] = around[r];
            }
        }
    }
    control.progress(0.7);
    control.check()?;
    // Components up to the minimum size are ignored.
    let roots = regions(&edges, &classes);
    let mut size = vec![0.0; n];
    for t in 0..n {
        if classes[t] != PART {
            size[roots[t] as usize] += areas[t];
        }
    }
    let mut up = SkinTotals {
        analyzed: s.upskins,
        angle: s.up_angle,
        ..SkinTotals::default()
    };
    let mut down = SkinTotals {
        analyzed: s.downskins,
        angle: s.down_angle,
        ..SkinTotals::default()
    };
    for t in 0..n {
        let r = roots[t] as usize;
        if classes[t] == PART {
            continue;
        }
        if size[r] <= s.min_component {
            classes[t] = PART;
            continue;
        }
        let totals = if classes[t] == UPSKIN {
            &mut up
        } else {
            &mut down
        };
        totals.area += areas[t];
        if r == t {
            totals.components += 1;
        }
    }
    control.progress(1.0);
    Ok(Skins { up, down, classes })
}
