//! Duplicate: copies of the selected parts, as one group, stacked on the originals, in a grid, or
//! filling the platform. Works on the group's world box and gives each copy a move from where the
//! originals are.

use serde::Deserialize;

use super::{Aabb, V3, size};

/// The most copies one Duplicate makes.
pub const MAX_COPIES: u64 = 10_000;

/// One axis of a grid: how many places along it (the originals' included), the spacing, and
/// whether the copies go towards the negative end.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GridAxis {
    pub count: u32,
    pub gap: f64,
    pub negative: bool,
}

/// What a grid's spacing measures: the gap between the copies' boxes, or the distance between
/// their origins (the same point of each copy).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Spacing {
    Boxes,
    Origins,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum Duplicate {
    /// Copies right on top of the originals.
    InPlace { copies: u32 },
    Grid {
        axes: [GridAxis; 3],
        spacing: Spacing,
    },
    /// As many as fit on the platform in a grid, `spacing` apart and `wall` from its sides,
    /// avoiding the other parts; in layers up to the platform's height with `stack`. At most `limit`
    /// copies. The originals move to the grid's first free place.
    Fill {
        spacing: f64,
        wall: f64,
        limit: u32,
        stack: bool,
    },
}

/// The result: how far the originals move (zero but for Fill) and each copy's move from where the
/// originals were.
#[derive(Clone, Debug, PartialEq)]
pub struct Copies {
    pub group: V3,
    pub offsets: Vec<V3>,
}

const EPS: f64 = 1e-9;

fn mm(x: f64) -> String {
    let s = format!("{x:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The copies of a group with world box `group`. `others` are the boxes of the parts that stay
/// where they are, which Fill keeps clear of.
pub fn duplicate(
    group: &Aabb,
    others: &[Aabb],
    platform: V3,
    how: &Duplicate,
) -> Result<Copies, String> {
    let s = size(group);
    match how {
        Duplicate::InPlace { copies } => {
            if *copies == 0 || u64::from(*copies) > MAX_COPIES {
                return Err(format!("Enter 1 to {MAX_COPIES} copies."));
            }
            Ok(Copies {
                group: [0.0; 3],
                offsets: vec![[0.0; 3]; *copies as usize],
            })
        }
        Duplicate::Grid { axes, spacing } => {
            let total: u64 = axes.iter().map(|a| u64::from(a.count)).product();
            if axes.iter().any(|a| a.count == 0) {
                return Err("Enter a count of at least 1 for each axis.".into());
            }
            if total < 2 {
                return Err("Enter a count above 1 for at least one axis.".into());
            }
            if total - 1 > MAX_COPIES {
                return Err(format!(
                    "That makes {} copies; at most {MAX_COPIES} are possible.",
                    total - 1
                ));
            }
            if axes.iter().any(|a| !a.gap.is_finite()) {
                return Err("Enter a number for each gap.".into());
            }
            let step: Vec<f64> = (0..3)
                .map(|k| {
                    let base = match spacing {
                        Spacing::Boxes => s[k] + axes[k].gap,
                        Spacing::Origins => axes[k].gap,
                    };
                    if axes[k].negative { -base } else { base }
                })
                .collect();
            let mut offsets = Vec::with_capacity((total - 1) as usize);
            for k in 0..axes[2].count {
                for j in 0..axes[1].count {
                    for i in 0..axes[0].count {
                        if (i, j, k) == (0, 0, 0) {
                            continue;
                        }
                        offsets.push([
                            f64::from(i) * step[0],
                            f64::from(j) * step[1],
                            f64::from(k) * step[2],
                        ]);
                    }
                }
            }
            Ok(Copies {
                group: [0.0; 3],
                offsets,
            })
        }
        Duplicate::Fill {
            spacing,
            wall,
            limit,
            stack,
        } => {
            let (spacing, wall) = (*spacing, *wall);
            if !(spacing.is_finite() && spacing >= 0.0) {
                return Err("Enter a distance between parts of 0 mm or more.".into());
            }
            if !(wall.is_finite() && wall >= 0.0) {
                return Err("Enter a distance to the platform's sides of 0 mm or more.".into());
            }
            let fit = |avail: f64, extent: f64| -> u64 {
                let step = extent + spacing;
                if extent > avail + EPS || step <= 0.0 {
                    0
                } else {
                    ((avail + spacing) / step + EPS).floor().min(1e6) as u64
                }
            };
            let nx = fit(platform[0] - 2.0 * wall, s[0]);
            let ny = fit(platform[1] - 2.0 * wall, s[1]);
            let nz = if *stack { fit(platform[2], s[2]) } else { 1 };
            if nx == 0 || ny == 0 || nz == 0 {
                return Err(format!(
                    "The selection ({} × {} × {} mm) does not fit on the platform with {} mm to its sides.",
                    mm(s[0]),
                    mm(s[1]),
                    mm(s[2]),
                    mm(wall)
                ));
            }
            let blocked = |cell: &Aabb| {
                others.iter().any(|o| {
                    (0..3).all(|a| {
                        cell[0][a] - spacing < o[1][a] - EPS && o[0][a] < cell[1][a] + spacing - EPS
                    })
                })
            };
            let wanted = u64::from(*limit).min(MAX_COPIES) + 1;
            let mut places = Vec::new();
            'cells: for k in 0..nz {
                for j in 0..ny {
                    for i in 0..nx {
                        let min = [
                            wall + i as f64 * (s[0] + spacing),
                            wall + j as f64 * (s[1] + spacing),
                            k as f64 * (s[2] + spacing),
                        ];
                        let cell = [min, [min[0] + s[0], min[1] + s[1], min[2] + s[2]]];
                        if blocked(&cell) {
                            continue;
                        }
                        places.push(min);
                        if places.len() as u64 >= wanted {
                            break 'cells;
                        }
                    }
                }
            }
            let Some(first) = places.first().copied() else {
                return Err(
                    "Other parts take up every place on the platform the selection would fit in."
                        .into(),
                );
            };
            let group_move = [
                first[0] - group[0][0],
                first[1] - group[0][1],
                first[2] - group[0][2],
            ];
            Ok(Copies {
                group: group_move,
                offsets: places
                    .iter()
                    .skip(1)
                    .map(|p| [p[0] - group[0][0], p[1] - group[0][1], p[2] - group[0][2]])
                    .collect(),
            })
        }
    }
}
