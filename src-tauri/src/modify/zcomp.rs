//! Z-compensation: the surfaces facing down are moved up by a distance, to make up for a printer
//! that cures or melts deeper than the layer it means to (energy reaching past the bottom of an
//! overhang). A triangle faces down when its outward normal points down and its surface is tilted
//! against the platform by at most the threshold angle; every point of such a triangle moves
//! straight up by the distance, the rest stay, so the walls round a downskin get shorter. The
//! triangles stay as they are: the part stays closed. A distance that would push a downskin
//! through the surface above it makes triangles cross, which `modify::run` refuses.

use serde::Deserialize;

use super::{Modified, triangles};
use crate::mesh::{Mesh, fmt_count};
use crate::repair::exact;
use crate::repair::{Control, RepairResult};

/// `ZCompSettings` in src/modify/types.ts.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ZCompSettings {
    /// mm, how far the downskins move up.
    pub distance: f64,
    /// Degrees: a surface facing down and tilted at most this much against the platform counts.
    pub angle: f64,
}

pub const MAX_DISTANCE: f64 = 10.0;

impl ZCompSettings {
    pub fn problem(&self) -> Option<String> {
        if !(self.distance.is_finite() && self.distance > 0.0 && self.distance <= MAX_DISTANCE) {
            return Some(format!(
                "Enter a distance greater than 0 and up to {MAX_DISTANCE} mm."
            ));
        }
        if !(self.angle.is_finite() && (0.0..90.0).contains(&self.angle)) {
            return Some("Enter a threshold angle from 0° to under 90°.".into());
        }
        None
    }
}

pub fn compensate(mesh: &Mesh, s: &ZCompSettings, control: &mut Control) -> RepairResult<Modified> {
    control.check()?;
    // Facing down and tilted at most `angle`: the normal within `angle` of straight down.
    let cos = s.angle.to_radians().cos();
    let mut lift = vec![false; mesh.vertex_count()];
    let mut down = 0usize;
    for t in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|v| exact::vertex(mesh, v));
        let n = exact::normal(a, b, c);
        let len = exact::length(n);
        if len > 0.0 && -n[2] >= cos * len - 1e-12 * len {
            down += 1;
            for &v in t {
                lift[v as usize] = true;
            }
        }
    }
    control.progress(0.5);
    if down == 0 {
        return Ok(Modified::unchanged(
            mesh,
            "No surface faces down within the threshold angle.",
        ));
    }
    let mut out = mesh.clone();
    let mut moved = 0usize;
    for (v, &up) in lift.iter().enumerate() {
        if up {
            let z = &mut out.positions[3 * v + 2];
            *z = (*z as f64 + s.distance) as f32;
            moved += 1;
        }
    }
    control.progress(1.0);
    Ok(Modified::moved(
        out,
        format!(
            "Moved {} facing down ({} {}) up by {} mm.",
            triangles(down),
            fmt_count(moved as u64),
            if moved == 1 { "point" } else { "points" },
            s.distance
        ),
    ))
}
