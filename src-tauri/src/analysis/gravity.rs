//! Centre of gravity: the centroid of the part's solid (uniform density), from the signed
//! tetrahedra its triangles make with a fixed point. Cavities facing inwards are taken out, so a
//! hollow part made by Generate shell has the centre of what is left.

use serde::Serialize;

use crate::arrange::{V3, World, add, centre, cross, dot, scale, sub};
use crate::repair::{Control, RepairResult};

/// `GravityResult` in src/analysis/types.ts. World mm.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gravity {
    pub centre: V3,
    /// The part's bounding box.
    pub min: V3,
    pub max: V3,
    pub size: V3,
    /// mm³ of the solid the centre is of.
    pub volume: f64,
}

/// The centre of gravity of the placed part; None when its triangles enclose no volume.
pub fn centre_of_gravity(world: &World, control: &mut Control) -> RepairResult<Option<Gravity>> {
    let Some(bounds) = world.bounds() else {
        return Ok(None);
    };
    // Tetrahedra from the box centre keep the sums small, so far-off parts lose no precision.
    let o = centre(&bounds);
    let mut volume = 0.0;
    let mut moment = [0.0; 3];
    for (i, &t) in world.triangles.iter().enumerate() {
        if i % 65_536 == 0 {
            control.check()?;
            control.progress(i as f64 / world.triangles.len() as f64);
        }
        let [a, b, c] = world.corners(t).map(|p| sub(p, o));
        let v = dot(a, cross(b, c)) / 6.0;
        volume += v;
        moment = add(moment, scale(add(add(a, b), c), v / 4.0));
    }
    control.progress(1.0);
    let size = sub(bounds[1], bounds[0]);
    let extent = size.iter().fold(0.0f64, |m, v| m.max(*v));
    // Also catches a volume that is not a number.
    let enclosed = volume.abs() > 1e-12 * extent.powi(3).max(f64::MIN_POSITIVE);
    if !enclosed {
        return Ok(None);
    }
    Ok(Some(Gravity {
        centre: add(o, scale(moment, 1.0 / volume)),
        min: bounds[0],
        max: bounds[1],
        size,
        volume: volume.abs(),
    }))
}
