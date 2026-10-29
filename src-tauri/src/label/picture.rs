//! Image labels: the parts of a picture darker than the threshold, as contours. The picture comes
//! as grey levels from the window, the way the Part library's Bitmap gets it (src/library/ipc.ts
//! pickPicture). Large pictures are first shrunk so their longer side has `MAX_SIDE` samples:
//! finer than any printer can raise.

use crate::library::Picture;

use super::region::P2;

/// Samples along the picture's longer side at most.
pub(crate) const MAX_SIDE: usize = 300;

/// The picture with its longer side at most `MAX_SIDE` samples, each the mean of the pixels it
/// covers.
pub(crate) fn shrunk(picture: &Picture) -> Picture {
    let (w, h) = (picture.width, picture.height);
    let longest = w.max(h);
    if longest <= MAX_SIDE {
        return Picture {
            width: w,
            height: h,
            gray: picture.gray.clone(),
        };
    }
    let scale = MAX_SIDE as f64 / longest as f64;
    let nw = ((w as f64 * scale).round() as usize).max(1);
    let nh = ((h as f64 * scale).round() as usize).max(1);
    let mut gray = vec![0f32; nw * nh];
    for j in 0..nh {
        let (y0, y1) = (j * h / nh, ((j + 1) * h / nh).max(j * h / nh + 1));
        for i in 0..nw {
            let (x0, x1) = (i * w / nw, ((i + 1) * w / nw).max(i * w / nw + 1));
            let mut sum = 0.0f64;
            for y in y0..y1.min(h) {
                for x in x0..x1.min(w) {
                    sum += picture.gray[y * w + x] as f64;
                }
            }
            gray[j * nw + i] = (sum / ((y1 - y0) * (x1 - x0)) as f64) as f32;
        }
    }
    Picture {
        width: nw,
        height: nh,
        gray,
    }
}

/// Contours round the samples darker than `threshold` (0 black to 1 white), anticlockwise round
/// dark areas and clockwise round light holes in them. Units are samples, y up: the picture's
/// top-left corner is at (0, height), its bottom-right at (width, 0). The outline runs between
/// samples where the grey level crosses the threshold, so edges follow the picture smoothly
/// rather than in pixel steps.
pub(crate) fn dark_contours(picture: &Picture, threshold: f64) -> Vec<Vec<P2>> {
    let (w, h) = (picture.width, picture.height);
    // Sample values on a grid one wider all round, the border light, so every contour closes.
    // Grid point (i, j) is sample (i − 1, j − 1) counted from the bottom row, at its centre.
    let (gw, gh) = (w + 2, h + 2);
    let value = |i: usize, j: usize| -> f64 {
        if i == 0 || j == 0 || i > w || j > h {
            return 1.0;
        }
        let row = h - j;
        let g = picture.gray[row * w + (i - 1)] as f64;
        if g.is_finite() { g - threshold } else { 1.0 }
    };
    let at = |i: usize, j: usize| [i as f64 - 0.5, j as f64 - 0.5];
    // Points on grid edges, keyed by edge: horizontal (i, j)–(i + 1, j) and vertical
    // (i, j)–(i, j + 1).
    let key = |vertical: bool, i: usize, j: usize| ((j * gw + i) * 2 + vertical as usize) as u64;
    let point = |vertical: bool, i: usize, j: usize| -> P2 {
        let (a, b) = if vertical {
            ((i, j), (i, j + 1))
        } else {
            ((i, j), (i + 1, j))
        };
        let (fa, fb) = (value(a.0, a.1), value(b.0, b.1));
        // Strictly between the samples, so no two contours meet at a sample.
        let t = if fa != fb {
            (fa / (fa - fb)).clamp(0.02, 0.98)
        } else {
            0.5
        };
        let (pa, pb) = (at(a.0, a.1), at(b.0, b.1));
        [pa[0] + t * (pb[0] - pa[0]), pa[1] + t * (pb[1] - pa[1])]
    };
    let mut next: std::collections::HashMap<u64, (u64, P2)> = std::collections::HashMap::new();
    for j in 0..gh - 1 {
        for i in 0..gw - 1 {
            // Corners anticlockwise from bottom left, and the edges between them: bottom, right,
            // top, left, each walked anticlockwise round the cell.
            let corners = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
            let dark = corners.map(|(x, y)| value(x, y) < 0.0);
            if dark.iter().all(|&d| d) || dark.iter().all(|&d| !d) {
                continue;
            }
            let edges = [
                (false, i, j),
                (true, i + 1, j),
                (false, i, j + 1),
                (true, i, j),
            ];
            // Walking anticlockwise round the cell, the outline leaves where an edge goes from
            // dark to light and comes back where one goes from light to dark; dark stays on its
            // left. At a saddle the middle decides whether the two dark corners join.
            let leaves: Vec<usize> = (0..4).filter(|&k| dark[k] && !dark[(k + 1) % 4]).collect();
            let joined = corners.iter().map(|&(x, y)| value(x, y)).sum::<f64>() < 0.0;
            for &k in &leaves {
                let mut m = k;
                loop {
                    m = if joined { (m + 1) % 4 } else { (m + 3) % 4 };
                    if !dark[m] && dark[(m + 1) % 4] {
                        break;
                    }
                }
                let (from, to) = (edges[k], edges[m]);
                let target = key(to.0, to.1, to.2);
                next.insert(
                    key(from.0, from.1, from.2),
                    (target, point(to.0, to.1, to.2)),
                );
            }
        }
    }
    let mut contours = Vec::new();
    let mut keys: Vec<u64> = next.keys().copied().collect();
    keys.sort_unstable();
    for start in keys {
        let Some(&(mut k, first)) = next.get(&start) else {
            continue;
        };
        let mut contour = vec![first];
        next.remove(&start);
        while k != start {
            let Some((to, p)) = next.remove(&k) else {
                break;
            };
            contour.push(p);
            k = to;
        }
        if contour.len() >= 3 {
            contours.push(simplified(&contour, 0.05));
        }
    }
    contours
}

/// The closed contour without the points that lie within `tolerance` of the line through their
/// neighbours (Douglas–Peucker on each half).
fn simplified(contour: &[P2], tolerance: f64) -> Vec<P2> {
    let n = contour.len();
    if n < 8 {
        return contour.to_vec();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n / 2] = true;
    let mut stack = vec![(0, n / 2), (n / 2, n)];
    while let Some((a, b)) = stack.pop() {
        if b - a < 2 {
            continue;
        }
        let (pa, pb) = (contour[a], contour[b % n]);
        let (dx, dy) = (pb[0] - pa[0], pb[1] - pa[1]);
        let len = dx.hypot(dy).max(1e-12);
        let (mut far, mut at) = (0.0, a);
        for (k, p) in contour.iter().enumerate().take(b).skip(a + 1) {
            let d = ((p[0] - pa[0]) * dy - (p[1] - pa[1]) * dx).abs() / len;
            if d > far {
                far = d;
                at = k;
            }
        }
        if far > tolerance {
            keep[at] = true;
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    contour
        .iter()
        .zip(keep)
        .filter_map(|(&p, k)| k.then_some(p))
        .collect()
}
