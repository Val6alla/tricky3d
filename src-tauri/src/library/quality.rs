//! Part library › Quality: test parts. The tensile, flexure and compression parts follow the ASTM
//! standards' dimensions (D638 types I to V, D790, D695); the microscale and multi-feature tests are
//! tricky3D's own designs for checking a printer's accuracy.

use std::f64::consts::PI;

use super::build::{Builder, P2, circle_2d, extrude};
use super::primitive_cylinder;
use super::solid::union;
use super::{Category, Item, Param, Polygon, Values, choice, count, item, mm};
use crate::mesh::{Mesh, make_box};
use crate::repair::{Control, RepairError, RepairResult};

fn quality(
    id: &'static str,
    label: &'static str,
    description: &'static str,
    params: Vec<Param>,
) -> Item {
    item(id, label, Category::Quality, description, params)
}

/// ASTM D638 dog-bone dimensions (mm): narrow width W, narrow length L, overall width WO, overall
/// length LO, fillet radius R.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DogBone {
    pub(crate) w: f64,
    pub(crate) l: f64,
    pub(crate) wo: f64,
    pub(crate) lo: f64,
    pub(crate) r: f64,
}

pub(crate) fn d638(kind: &str) -> DogBone {
    let (w, l, wo, lo, r) = match kind {
        "II" => (6.0, 57.0, 19.0, 183.0, 76.0),
        "III" => (19.0, 57.0, 29.0, 246.0, 76.0),
        "IV" => (6.0, 33.0, 19.0, 115.0, 14.0),
        "V" => (3.18, 9.53, 9.53, 63.5, 12.7),
        _ => (13.0, 57.0, 19.0, 165.0, 76.0),
    };
    DogBone { w, l, wo, lo, r }
}

impl DogBone {
    /// How far each fillet reaches along the bar.
    pub(crate) fn fillet_length(&self) -> f64 {
        let step = (self.wo - self.w) / 2.0;
        (self.r * self.r - (self.r - step).powi(2)).sqrt()
    }

    /// The outline's area: two grips, the narrow part, and four fillet transitions.
    #[cfg(test)]
    pub(crate) fn area(&self) -> f64 {
        let e = self.fillet_length();
        let grip = (self.lo - self.l - 2.0 * e) / 2.0;
        let r = self.r;
        // ∫ from 0 to e of (W + 2R − 2√(R² − u²)) du, for each end.
        let transition =
            (self.w + 2.0 * r) * e - e * (r * r - e * e).sqrt() - r * r * (e / r).asin();
        2.0 * grip * self.wo + self.l * self.w + 2.0 * transition
    }

    /// The outline anticlockwise, centred on the origin, along X.
    pub(crate) fn outline(&self, arc_points: usize) -> Vec<P2> {
        let (l, w, wo, lo, r) = (self.l, self.w, self.wo, self.lo, self.r);
        let e = self.fillet_length();
        let step = (wo - w) / 2.0;
        // The fillet's angles round its centre (below the narrow edge on the bottom side).
        let start = (r - step).atan2(-e);
        let mut bottom: Vec<P2> = vec![[-lo / 2.0, -wo / 2.0], [-l / 2.0 - e, -wo / 2.0]];
        for k in 1..=arc_points {
            let a = start + (PI / 2.0 - start) * k as f64 / arc_points as f64;
            bottom.push([-l / 2.0 + r * a.cos(), -w / 2.0 - r + r * a.sin()]);
        }
        // The right half mirrors the left.
        let left: Vec<P2> = bottom.clone();
        for p in left.iter().rev() {
            bottom.push([-p[0], p[1]]);
        }
        bottom.dedup();
        let lower = bottom.clone();
        let mut out = lower.clone();
        for p in lower.iter().rev() {
            out.push([p[0], -p[1]]);
        }
        out.dedup();
        out
    }
}

pub(super) fn catalog() -> Vec<Item> {
    vec![
        quality(
            "tensile",
            "Tensile Test Part",
            "A dog-bone bar for pulling apart, to ASTM D638.",
            vec![
                choice(
                    "spec",
                    "Specification",
                    "I",
                    &[
                        ("I", "Type I (165 mm)"),
                        ("II", "Type II (183 mm)"),
                        ("III", "Type III (246 mm)"),
                        ("IV", "Type IV (115 mm)"),
                        ("V", "Type V (63.5 mm)"),
                    ],
                ),
                mm("thickness", "Thickness", 3.2)
                    .help("The standard asks for 7 mm or less (Type III: 7 to 14 mm)."),
            ],
        ),
        quality(
            "flexure",
            "Flexure Test Part",
            "A bar for bending, to ASTM D790 (127 × 12.7 × 3.2 mm).",
            vec![choice(
                "spec",
                "Specification",
                "d790",
                &[
                    ("d790", "D790 bar 127 × 12.7 × 3.2 mm"),
                    ("d790_thick", "D790 bar 127 × 12.7 × 6.4 mm"),
                ],
            )],
        ),
        quality(
            "compression",
            "Compression Test Part",
            "A block for crushing, to ASTM D695.",
            vec![
                choice(
                    "spec",
                    "Specification",
                    "cylinder",
                    &[
                        ("cylinder", "Cylinder 12.7 mm across, 25.4 mm high"),
                        ("prism", "Prism 12.7 × 12.7 × 25.4 mm"),
                    ],
                ),
                count("segments", "Segments", 96.0, 8.0, 2048.0),
            ],
        ),
        quality(
            "nist",
            "NIST Test Design",
            "The NIST additive manufacturing test artifact.",
            Vec::new(),
        )
        .soon("tricky3D does not have the published artifact's exact dimensions to build it from."),
        quality(
            "microscale",
            "Microscale test",
            "Rows of ever finer pins, walls and holes, to see the smallest your printer makes.",
            vec![
                mm("smallest", "Smallest feature", 0.2),
                mm("step", "Step", 0.1),
                count("steps", "Steps", 6.0, 2.0, 12.0),
            ],
        ),
        quality(
            "multi_feature",
            "Multi-Feature Test",
            "Walls, pins, holes, overhangs and bridges in steps on one plate.",
            vec![count("steps", "Steps", 5.0, 3.0, 8.0)],
        ),
        quality(
            "test_rod",
            "Test rod",
            "A plain round rod.",
            vec![
                mm("diameter", "Diameter", 5.0),
                mm("length", "Length", 50.0),
                count("segments", "Segments", 64.0, 8.0, 2048.0),
            ],
        ),
    ]
}

pub(super) fn make(id: &str, v: &Values, control: &mut Control) -> RepairResult<Mesh> {
    let failed = RepairError::Failed;
    match id {
        "tensile" => {
            let bone = d638(v.choice("spec").map_err(failed)?);
            let mut b = Builder::default();
            extrude(
                &mut b,
                &[Polygon::new(bone.outline(48), Vec::new())],
                0.0,
                v.get("thickness").map_err(failed)?,
            )
            .map_err(failed)?;
            b.finish().map_err(failed)
        }
        "flexure" => {
            let t = if v.choice("spec").map_err(failed)? == "d790_thick" {
                6.4
            } else {
                3.2
            };
            Ok(block(127.0, 12.7, t))
        }
        "compression" => {
            if v.choice("spec").map_err(failed)? == "prism" {
                Ok(block(12.7, 12.7, 25.4))
            } else {
                primitive_cylinder(12.7 / 2.0, 25.4, v.count("segments").map_err(failed)?)
                    .map_err(failed)
            }
        }
        "test_rod" => primitive_cylinder(
            v.get("diameter").map_err(failed)? / 2.0,
            v.get("length").map_err(failed)?,
            v.count("segments").map_err(failed)?,
        )
        .map_err(failed),
        "microscale" => microscale(v, control),
        "multi_feature" => multi_feature(v, control),
        _ => unreachable!("every quality item has a generator"),
    }
}

/// A box centred on the Z axis standing on z = 0.
fn block(x: f64, y: f64, z: f64) -> Mesh {
    make_box(
        [(-x / 2.0) as f32, (-y / 2.0) as f32, 0.0],
        [(x / 2.0) as f32, (y / 2.0) as f32, z as f32],
    )
}

fn boxed(x0: f64, y0: f64, z0: f64, x1: f64, y1: f64, z1: f64) -> Mesh {
    make_box(
        [x0 as f32, y0 as f32, z0 as f32],
        [x1 as f32, y1 as f32, z1 as f32],
    )
}

/// A round pin standing at (x, y) from z0 to z1.
fn pin(x: f64, y: f64, d: f64, z0: f64, z1: f64) -> Result<Mesh, String> {
    let mut b = Builder::default();
    extrude(
        &mut b,
        &[Polygon::new(
            circle_2d([x, y], d / 2.0, 32, 0.0),
            Vec::new(),
        )],
        z0,
        z1,
    )?;
    b.finish_in_place()
}

/// A plate from (x0, y0) to (x1, y1), z0 to z1, with round holes (centre, diameter) through it.
fn holed_plate(
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    z0: f64,
    z1: f64,
    holes: &[(P2, f64)],
) -> Result<Mesh, String> {
    let outline = vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    let loops = holes
        .iter()
        .map(|&(c, d)| circle_2d(c, d / 2.0, 32, 0.0))
        .collect();
    let mut b = Builder::default();
    extrude(&mut b, &[Polygon::new(outline, loops)], z0, z1)?;
    b.finish_in_place()
}

fn stand(mesh: Mesh) -> RepairResult<Mesh> {
    let mut b = Builder::default();
    b.append(&mesh);
    b.finish().map_err(RepairError::Failed)
}

/// Three rows on a plate: pins, thin walls, and holes through the plate, each feature one step
/// bigger than the one before.
fn microscale(v: &Values, control: &mut Control) -> RepairResult<Mesh> {
    let failed = RepairError::Failed;
    let (smallest, step, n) = (
        v.get("smallest").map_err(failed)?,
        v.get("step").map_err(failed)?,
        v.count("steps").map_err(failed)?,
    );
    let largest = smallest + step * (n - 1) as f64;
    let pitch = (largest + 1.5).max(2.0);
    let (base, height) = (1.2, 3.0);
    let length = pitch * n as f64 + 2.0;
    let width = 3.0 * (largest + 4.0) + 2.0;
    let size = |k: usize| smallest + step * k as f64;
    let x = |k: usize| 1.0 + pitch * (k as f64 + 0.5);
    let row = |r: usize| 1.0 + (largest + 4.0) * (r as f64 + 0.5);
    let holes: Vec<(P2, f64)> = (0..n).map(|k| ([x(k), row(2)], size(k))).collect();
    let mut parts = vec![holed_plate(0.0, 0.0, length, width, 0.0, base, &holes).map_err(failed)?];
    for k in 0..n {
        parts.push(pin(x(k), row(0), size(k), base - 0.1, base + height).map_err(failed)?);
        let t = size(k);
        parts.push(boxed(
            x(k) - t / 2.0,
            row(1) - 2.0,
            base - 0.1,
            x(k) + t / 2.0,
            row(1) + 2.0,
            base + height,
        ));
    }
    stand(union(&parts, control)?)
}

/// A 70 × 50 mm plate with rows of walls and pins in steps, a block with holes in steps,
/// overhangs at 60°, 45° and 30° from the horizontal, and bridges of growing span.
fn multi_feature(v: &Values, control: &mut Control) -> RepairResult<Mesh> {
    let failed = RepairError::Failed;
    let n = v.count("steps").map_err(failed)?;
    let (l, w, base) = (70.0, 50.0, 2.0);
    let top = base - 0.1;
    let mut parts = vec![boxed(0.0, 0.0, 0.0, l, w, base)];
    let along = |k: usize, from: f64, to: f64| from + (to - from) * (k as f64 + 0.5) / n as f64;
    // Thin walls 0.4 mm and up, 8 mm high.
    for k in 0..n {
        let t = 0.4 + 0.2 * k as f64;
        let x = along(k, 3.0, 33.0);
        parts.push(boxed(x - t / 2.0, 3.0, top, x + t / 2.0, 11.0, base + 8.0));
    }
    // Pins 1 mm and up across, 8 mm high.
    for k in 0..n {
        let d = 1.0 + 0.5 * k as f64;
        parts.push(pin(along(k, 3.0, 33.0), 16.0, d, top, base + 8.0).map_err(failed)?);
    }
    // A block with holes 1 mm and up across.
    let holes: Vec<(P2, f64)> = (0..n)
        .map(|k| ([along(k, 3.0, 33.0), 25.0], 1.0 + 0.5 * k as f64))
        .collect();
    parts.push(holed_plate(2.0, 21.0, 34.0, 29.0, top, base + 5.0, &holes).map_err(failed)?);
    // Overhangs: walls leaning out at 60°, 45° and 30° from the horizontal, 6 mm high.
    for (k, angle) in [60.0f64, 45.0, 30.0].into_iter().enumerate() {
        let y0 = 3.0 + 9.0 * k as f64;
        let h = 6.0;
        let lean = h / angle.to_radians().tan();
        let outline = vec![
            [40.0, top],
            [44.0, top],
            [44.0 + lean, base + h],
            [40.0, base + h],
        ];
        // The profile is in X and Z; extruded along Y.
        let mut b = Builder::default();
        extrude(&mut b, &[Polygon::new(outline, Vec::new())], y0, y0 + 6.0).map_err(failed)?;
        let side = b.finish_in_place().map_err(failed)?;
        // (x, z, y) → (x, y, z), turned so the extrusion runs along Y.
        let mut turned = side.clone();
        for p in turned.positions.as_chunks_mut::<3>().0 {
            *p = [p[0], p[2], p[1]];
        }
        for t in turned.indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
        parts.push(turned);
    }
    // Bridges: two pillars with a slab across, spans 4 mm and up.
    for k in 0..n.min(5) {
        let span = 4.0 + 4.0 * k as f64;
        let y = 32.0 + 3.5 * k as f64;
        let x0 = 66.0 - span - 6.0;
        parts.push(boxed(x0, y, top, x0 + 3.0, y + 2.5, base + 6.0));
        parts.push(boxed(
            x0 + 3.0 + span,
            y,
            top,
            x0 + 6.0 + span,
            y + 2.5,
            base + 6.0,
        ));
        parts.push(boxed(
            x0 + 0.5,
            y,
            base + 5.9,
            x0 + 5.5 + span,
            y + 2.5,
            base + 7.0,
        ));
    }
    let _ = w;
    stand(union(&parts, control)?)
}
