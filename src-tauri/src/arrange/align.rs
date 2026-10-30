//! Arrange › Align: parts lined up with each other (by the box round all of them), or put on the
//! platform. Works on the parts' world boxes and gives each part a move.

use serde::Deserialize;

use super::{Aabb, V3, centre, union_all};

/// How to align. "Front" is the platform's near edge, the smallest Y.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Align {
    Left,
    Right,
    Front,
    Back,
    Bottom,
    Top,
    CentreX,
    CentreY,
    CentreZ,
    /// The parts as one group, centred on the platform in X and Y.
    PlatformCentre,
    /// The parts as one group, their box's smallest corner on the platform's origin.
    Origin,
    /// Each part on its own, its lowest point on the platform (Z = 0).
    Drop,
}

impl Align {
    /// Lines parts up with each other, so it needs two or more.
    pub fn relative(self) -> bool {
        !matches!(self, Align::PlatformCentre | Align::Origin | Align::Drop)
    }
}

/// The move of each part, in the order of `boxes`. Parts that are already in place get a zero move.
pub fn align(boxes: &[Aabb], how: Align, platform: V3) -> Vec<V3> {
    let Some(all) = union_all(boxes) else {
        return Vec::new();
    };
    let to = |axis: usize, value: f64, of: f64| {
        let mut d = [0.0; 3];
        d[axis] = value - of;
        d
    };
    boxes
        .iter()
        .map(|b| match how {
            Align::Left => to(0, all[0][0], b[0][0]),
            Align::Right => to(0, all[1][0], b[1][0]),
            Align::Front => to(1, all[0][1], b[0][1]),
            Align::Back => to(1, all[1][1], b[1][1]),
            Align::Bottom => to(2, all[0][2], b[0][2]),
            Align::Top => to(2, all[1][2], b[1][2]),
            Align::CentreX => to(0, centre(&all)[0], centre(b)[0]),
            Align::CentreY => to(1, centre(&all)[1], centre(b)[1]),
            Align::CentreZ => to(2, centre(&all)[2], centre(b)[2]),
            Align::PlatformCentre => {
                let c = centre(&all);
                [platform[0] / 2.0 - c[0], platform[1] / 2.0 - c[1], 0.0]
            }
            Align::Origin => [-all[0][0], -all[0][1], -all[0][2]],
            Align::Drop => to(2, 0.0, b[0][2]),
        })
        .collect()
}
