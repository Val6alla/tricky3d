//! Spur gears with involute teeth: module m, z teeth, pressure angle α. The pitch circle has radius
//! m·z/2, the tips reach m beyond it (outside diameter m·(z + 2)) and the roots 1.25·m within it.
//! Each flank is the involute of the base circle (radius m·z/2·cos α) from the base circle (or the
//! root, whichever is further out) to the tip; below the base circle a radial line runs down to
//! the root. The tooth is m·π/2 thick on the pitch circle, less the backlash.

use std::f64::consts::{PI, TAU};

use super::Polygon;
use super::build::{Builder, P2, circle_2d, extrude};
use crate::mesh::Mesh;

/// tan α − α.
fn involute(alpha: f64) -> f64 {
    alpha.tan() - alpha
}

pub(crate) struct Gear {
    pub(crate) module: f64,
    pub(crate) teeth: usize,
    pub(crate) pressure_deg: f64,
    pub(crate) backlash: f64,
    /// Points along each flank.
    pub(crate) flank_points: usize,
}

impl Gear {
    pub(crate) fn pitch_radius(&self) -> f64 {
        self.module * self.teeth as f64 / 2.0
    }

    pub(crate) fn tip_radius(&self) -> f64 {
        self.pitch_radius() + self.module
    }

    pub(crate) fn root_radius(&self) -> f64 {
        self.pitch_radius() - 1.25 * self.module
    }

    /// The outline anticlockwise, tooth after tooth, the first tooth centred on +X.
    pub(crate) fn outline(&self) -> Result<Vec<P2>, String> {
        let r = self.pitch_radius();
        let alpha = self.pressure_deg.to_radians();
        let rb = r * alpha.cos();
        let (ra, rf) = (self.tip_radius(), self.root_radius());
        if rf <= 0.0 {
            return Err("A gear needs at least 3 teeth.".into());
        }
        // Half the tooth's angle on the pitch circle.
        let half = (PI * self.module / 2.0 - self.backlash) / (2.0 * r);
        if half <= 0.0 {
            return Err("The backlash leaves no tooth.".into());
        }
        // A flank point's angle from the tooth's middle at radius ρ ≥ rb.
        let angle_at = |rho: f64| half + involute(alpha) - involute((rb / rho).min(1.0).acos());
        let start = rb.max(rf);
        let tip_half = angle_at(ra);
        if tip_half <= 0.0 {
            return Err(
                "The teeth would come to a point before the tip circle; use fewer teeth or a larger pressure angle."
                    .into(),
            );
        }
        let base_half = angle_at(start);
        let pitch_angle = TAU / self.teeth as f64;
        if 2.0 * base_half >= pitch_angle {
            return Err("The teeth would run into each other at the root.".into());
        }
        let n = self.flank_points.max(3);
        let mut out = Vec::new();
        for k in 0..self.teeth {
            let c = k as f64 * pitch_angle;
            let at = |rho: f64, a: f64| [rho * a.cos(), rho * a.sin()];
            // Root arc from the middle of the gap before this tooth to the bottom of its flank.
            let gap_middle = c - pitch_angle / 2.0;
            let root_steps = 4;
            for s in 0..root_steps {
                let a = gap_middle + (c - base_half - gap_middle) * s as f64 / root_steps as f64;
                out.push(at(rf, a));
            }
            // Up the flank that faces backwards (clockwise side), from the root.
            if rf < rb {
                out.push(at(rf, c - base_half));
            }
            for s in 0..n {
                let rho = start + (ra - start) * s as f64 / (n - 1) as f64;
                out.push(at(rho, c - angle_at(rho)));
            }
            // Across the tip, through its middle.
            let tip_steps = 4;
            for s in 1..tip_steps {
                let a = c - tip_half + 2.0 * tip_half * s as f64 / tip_steps as f64;
                out.push(at(ra, a));
            }
            // Down the other flank to the root.
            for s in (0..n).rev() {
                let rho = start + (ra - start) * s as f64 / (n - 1) as f64;
                out.push(at(rho, c + angle_at(rho)));
            }
            if rf < rb {
                out.push(at(rf, c + base_half));
            }
            // The rest of the root arc to the next gap's middle comes with the next tooth.
            for s in 1..root_steps {
                let end = c + pitch_angle / 2.0;
                let a = c + base_half + (end - c - base_half) * s as f64 / root_steps as f64;
                out.push(at(rf, a));
            }
        }
        Ok(out)
    }
}

/// A gear `thickness` thick with a round bore, and for a light gear `holes` round holes between the
/// hub and the rim.
pub(crate) fn gear_mesh(
    gear: &Gear,
    thickness: f64,
    bore: f64,
    holes: usize,
) -> Result<Mesh, String> {
    let outline = gear.outline()?;
    let rf = gear.root_radius();
    let mut hole_loops: Vec<Vec<P2>> = Vec::new();
    let segments = 64;
    if bore > 0.0 {
        if bore / 2.0 >= rf - gear.module.max(1.0) {
            return Err(format!(
                "The bore must be smaller than {:.2} mm across for this gear.",
                2.0 * (rf - gear.module.max(1.0))
            ));
        }
        hole_loops.push(circle_2d([0.0, 0.0], bore / 2.0, segments, 0.0));
    }
    if holes > 0 {
        // A hub round the bore and a rim inside the teeth, each at least two modules (or 2 mm).
        let hub = bore / 2.0 + (2.0 * gear.module).max(2.0);
        let rim = rf - (2.0 * gear.module).max(2.0);
        if rim - hub < 2.0 {
            return Err(
                "There is no room for lightening holes; use more teeth or a smaller bore.".into(),
            );
        }
        let ring = (hub + rim) / 2.0;
        // Round holes that leave a spoke of at least a third of their size between them.
        let across = (rim - hub).min(TAU * ring / holes as f64 * 0.75);
        let r = across / 2.0;
        for k in 0..holes {
            let a = TAU * (k as f64 + 0.5) / holes as f64;
            hole_loops.push(circle_2d([ring * a.cos(), ring * a.sin()], r, 32, a));
        }
    }
    let mut b = Builder::default();
    extrude(&mut b, &[Polygon::new(outline, hole_loops)], 0.0, thickness)?;
    b.finish()
}
