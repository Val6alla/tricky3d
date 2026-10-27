//! The Status tab: "Mesh is closed", "Mesh is oriented", the statistics, and what the viewport
//! highlights (holes, inverted triangles, degenerate faces, non-manifold edges). Also the damage
//! badge check in the parts tree.

use serde::Serialize;

use super::defaults::{DEGENERATE, effective_degenerate_tolerance};
use super::exact::collinear;
use super::orient;
use super::topology::{Edges, Link, boundary_loops, components, orientation_patches};
use super::{Control, RepairResult};
use crate::mesh::{Mesh, cross, dot, sub};

/// `RepairStatus` in src/repair/types.ts.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairStatus {
    /// Undirected edges.
    pub edges: u64,
    pub triangles: u64,
    pub vertices: u64,
    /// Shells (`Link::Shell`, as the info panel counts them).
    pub shells: u64,
    /// Edges with exactly one triangle.
    pub border_edges: u64,
    /// Edges with three or more triangles.
    pub non_manifold_edges: u64,
    /// Triangles whose orientation disagrees with their shell (orient::inverted).
    pub inverted_triangles: u64,
    /// Border loops.
    pub holes: u64,
    /// Triangles below the Remove degenerate faces default tolerance.
    pub degenerate_triangles: u64,
    /// No border edges and no non-manifold edges (and at least one triangle).
    pub closed: bool,
    /// No inverted triangles and no edge whose two triangles cannot agree.
    pub oriented: bool,
    /// mm³, signed (positive when the closed shells face outward).
    pub volume: f64,
    /// mm².
    pub area: f64,
}

/// The status plus the data the viewport draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatusDetail {
    pub status: RepairStatus,
    /// x, y, z, x, y, z per border edge.
    pub border: Vec<f32>,
    /// x, y, z, x, y, z per non-manifold edge.
    pub non_manifold: Vec<f32>,
    /// Inverted triangles, sorted.
    pub inverted: Vec<u32>,
    /// Degenerate triangles, sorted.
    pub degenerate: Vec<u32>,
}

/// Edges, triangles, vertices, shells, border and non-manifold edges count as the info panel
/// counts them (src/mesh.rs `analyze`), and the edge segments come in its order; holes are the
/// border loops (topology::boundary_loops); inverted triangles are orient::inverted's with nested
/// shells taken as most of each faces (an inward cavity is not inverted); degenerate triangles are measured at the Remove degenerate faces default
/// tolerance for this part's size.
pub fn analyze(mesh: &Mesh, control: &mut Control) -> RepairResult<StatusDetail> {
    control.check()?;
    let edges = Edges::new(mesh);
    control.progress(0.25);
    control.check()?;
    let mut border = Vec::new();
    let mut non_manifold = Vec::new();
    for e in 0..edges.len() as u32 {
        let segments = match edges.degree(e) {
            2 => continue,
            1 => &mut border,
            _ => &mut non_manifold,
        };
        for v in edges.ends(e) {
            segments.extend_from_slice(&mesh.vertex(v));
        }
    }
    let holes = boundary_loops(mesh, &edges).len();
    let (_, shells) = components(mesh, &edges, Link::Shell);
    control.progress(0.4);
    control.check()?;
    let patches = orientation_patches(mesh, &edges);
    control.progress(0.55);
    let flipped = orient::inverted(
        mesh,
        &edges,
        &patches,
        orient::Nested::AsMost,
        &mut control.sub(0.55, 0.85),
    )?;
    let inverted: Vec<u32> = (0..flipped.len() as u32)
        .filter(|&t| flipped[t as usize])
        .collect();
    control.check()?;
    let tolerance = effective_degenerate_tolerance(&DEGENERATE);
    let degenerate = degenerate_triangles(mesh, tolerance);
    let (volume, area) = volume_and_area(mesh);
    control.progress(1.0);
    let triangles = mesh.triangle_count() as u64;
    let status = RepairStatus {
        edges: edges.len() as u64,
        triangles,
        vertices: mesh.vertex_count() as u64,
        shells: shells as u64,
        border_edges: (border.len() / 6) as u64,
        non_manifold_edges: (non_manifold.len() / 6) as u64,
        inverted_triangles: inverted.len() as u64,
        holes: holes as u64,
        degenerate_triangles: degenerate.len() as u64,
        closed: triangles > 0 && border.is_empty() && non_manifold.is_empty(),
        oriented: inverted.is_empty() && patches.conflicts == 0,
        volume,
        area,
    };
    Ok(StatusDetail {
        status,
        border,
        non_manifold,
        inverted,
        degenerate,
    })
}

/// The triangles that are degenerate at `tolerance` mm: two corners closer than it (the shortest
/// edge below it), or no area at all. Sorted. A thin triangle whose corners all lie far apart is
/// not degenerate: the reference tool's results keep them (see defaults::DEGENERATE).
pub fn degenerate_triangles(mesh: &Mesh, tolerance: f64) -> Vec<u32> {
    mesh.indices
        .as_chunks::<3>()
        .0
        .iter()
        .enumerate()
        .filter(|&(_, &t)| degenerate(mesh, t, tolerance))
        .map(|(t, _)| t as u32)
        .collect()
}

fn degenerate(mesh: &Mesh, [i, j, k]: [u32; 3], tolerance: f64) -> bool {
    if i == j || j == k || k == i {
        return true;
    }
    let (a, b, c) = (mesh.point(i), mesh.point(j), mesh.point(k));
    let lengths = [sub(b, a), sub(c, b), sub(a, c)].map(|d| dot(d, d));
    let shortest = lengths[0].min(lengths[1]).min(lengths[2]).sqrt();
    let longest = lengths[0].max(lengths[1]).max(lengths[2]).sqrt();
    let n = cross(sub(b, a), sub(c, a));
    let twice_area = dot(n, n).sqrt();
    shortest < tolerance - f32_slack([a, b, c])
        // Exactly zero area. Rounding moves twice the area by far less than 1e-9 × longest², so
        // above that it cannot be zero.
        || (twice_area <= 1e-9 * longest * longest && collinear(a, b, c))
}

/// How far storing a triangle's corners in f32 can move its edge lengths: √3 units in the last
/// place of its largest coordinate. An edge counts as shorter than the tolerance only when it is
/// shorter by more than that, so an edge one tolerance long, as Wrap part surface leaves them on its
/// 0.002 mm grid, is not degenerate at a tolerance of 0.002 mm (the reference tool's results keep
/// such edges too).
pub(crate) fn f32_slack(corners: [[f64; 3]; 3]) -> f64 {
    let largest = corners
        .iter()
        .flatten()
        .fold(0.0f64, |m, x| m.max(x.abs()))
        .max(f32::MIN_POSITIVE as f64) as f32;
    3f64.sqrt() * (f32::from_bits(largest.to_bits() + 1) - largest) as f64
}

/// Signed volume and area summed as src/mesh.rs sums them for the info panel, so both show the
/// same numbers.
fn volume_and_area(mesh: &Mesh) -> (f64, f64) {
    let mut volume = 0.0;
    let mut area = 0.0;
    for t in mesh.indices.as_chunks::<3>().0 {
        let a = mesh.point(t[0]);
        let b = mesh.point(t[1]);
        let c = mesh.point(t[2]);
        volume += dot(a, cross(b, c));
        let n = cross(sub(b, a), sub(c, a));
        area += dot(n, n).sqrt();
    }
    (volume / 6.0, area / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{analyze as measure, make_box};
    use crate::ops::concat;
    use crate::repair::silent;
    use crate::repair::topology::bbox_diagonal;
    use std::time::Instant;

    fn cube() -> Mesh {
        make_box([0.0; 3], [20.0; 3])
    }

    /// One triangle with corners at these grid steps of 0.002 mm, stored in f32 as Wrap part
    /// surface stores them.
    fn on_grid(steps: [[i64; 3]; 3]) -> Mesh {
        Mesh {
            positions: steps
                .iter()
                .flat_map(|p| p.map(|k| (k as f64 * 0.002) as f32))
                .collect(),
            indices: vec![0, 1, 2],
        }
    }

    #[test]
    fn an_edge_one_grid_step_long_is_not_degenerate_at_that_tolerance() {
        // Far from the origin a step stored in f32 measures a hair more or less than 0.002 mm.
        let mut short_in_f32 = 0;
        for base in [0i64, 7, 5_000, 61_234, 391_517] {
            for axis in 0..3 {
                let mut b = [base; 3];
                b[axis] += 1;
                let mut c = [base; 3];
                c[(axis + 1) % 3] += 40;
                let m = on_grid([[base; 3], b, c]);
                let d = sub(m.point(1), m.point(0));
                let step = dot(d, d).sqrt();
                short_in_f32 += (step < 0.002) as usize;
                assert!(
                    degenerate_triangles(&m, 0.002).is_empty(),
                    "{base} {axis}: {step}"
                );
            }
        }
        assert!(short_in_f32 > 0, "every step measured 0.002 or more");
        // An edge clearly shorter still counts, at any distance from the origin.
        let m = Mesh {
            positions: vec![780.0, 0.0, 0.0, 780.0019, 0.0, 0.0, 780.0, 0.08, 0.0],
            indices: vec![0, 1, 2],
        };
        assert_eq!(degenerate_triangles(&m, 0.002), vec![0]);
        let m = Mesh {
            positions: vec![1.0, 0.0, 0.0, 1.0019, 0.0, 0.0, 1.0, 0.08, 0.0],
            indices: vec![0, 1, 2],
        };
        assert_eq!(degenerate_triangles(&m, 0.002), vec![0]);
    }

    fn flip(mesh: &mut Mesh, t: usize) {
        mesh.indices.swap(t * 3 + 1, t * 3 + 2);
    }

    fn inside_out(mut mesh: Mesh) -> Mesh {
        for t in 0..mesh.triangle_count() {
            flip(&mut mesh, t);
        }
        mesh
    }

    fn detail(mesh: &Mesh) -> StatusDetail {
        let detail = silent(|control| analyze(mesh, control)).unwrap();
        agrees_with_the_info_panel(mesh, &detail);
        detail
    }

    fn status(mesh: &Mesh) -> RepairStatus {
        detail(mesh).status
    }

    /// The counts the info panel also shows must be the same numbers, and the highlighted edge
    /// segments must be the info panel's problem edges.
    fn agrees_with_the_info_panel(mesh: &Mesh, detail: &StatusDetail) {
        let a = measure(mesh, &mut |_| {});
        let s = &detail.status;
        assert_eq!(s.triangles, a.stats.triangles);
        assert_eq!(s.vertices, a.stats.vertices);
        assert_eq!(s.shells, a.stats.shells);
        assert_eq!(s.border_edges, a.stats.open_edges);
        assert_eq!(s.non_manifold_edges, a.stats.non_manifold_edges);
        assert_eq!(s.volume, a.stats.volume);
        assert_eq!(s.area, a.stats.area);
        let mut ours: Vec<[u32; 6]> = detail
            .border
            .as_chunks::<6>()
            .0
            .iter()
            .chain(detail.non_manifold.as_chunks::<6>().0)
            .map(|c| c.map(f32::to_bits))
            .collect();
        let mut theirs: Vec<[u32; 6]> = a
            .problem_edges
            .as_chunks::<6>()
            .0
            .iter()
            .map(|c| c.map(f32::to_bits))
            .collect();
        ours.sort_unstable();
        theirs.sort_unstable();
        assert_eq!(ours, theirs);
    }

    #[test]
    fn a_cube_has_no_problems() {
        let d = detail(&cube());
        assert_eq!(
            d.status,
            RepairStatus {
                edges: 18,
                triangles: 12,
                vertices: 8,
                shells: 1,
                border_edges: 0,
                non_manifold_edges: 0,
                inverted_triangles: 0,
                holes: 0,
                degenerate_triangles: 0,
                closed: true,
                oriented: true,
                volume: 8000.0,
                area: 2400.0,
            }
        );
        assert!(d.border.is_empty() && d.non_manifold.is_empty());
        assert!(d.inverted.is_empty() && d.degenerate.is_empty());
    }

    #[test]
    fn an_empty_mesh_is_not_closed() {
        let s = status(&Mesh::default());
        assert!(!s.closed && s.oriented);
        assert_eq!((s.edges, s.triangles, s.shells, s.holes), (0, 0, 0, 0));
    }

    #[test]
    fn a_cube_missing_a_face_has_one_hole() {
        let mut m = cube();
        m.indices.drain(0..6);
        let d = detail(&m);
        let s = &d.status;
        // The open face's diagonal is gone; its four sides are border edges now.
        assert_eq!((s.holes, s.border_edges, s.edges), (1, 4, 17));
        assert!(!s.closed && s.oriented);
        assert_eq!(s.inverted_triangles, 0);
        assert_eq!(d.border.len(), 4 * 6);
        assert!(d.border.as_chunks::<3>().0.iter().all(|p| p[2] == 0.0));
    }

    #[test]
    fn three_turned_triangles_are_counted_and_listed() {
        let mut m = cube();
        for t in [1, 6, 9] {
            flip(&mut m, t);
        }
        let d = detail(&m);
        assert_eq!(d.status.inverted_triangles, 3);
        assert_eq!(d.inverted, vec![1, 6, 9]);
        assert!(d.status.closed && !d.status.oriented);
    }

    #[test]
    fn an_inside_out_cube_is_all_inverted() {
        let s = status(&inside_out(cube()));
        assert_eq!(s.inverted_triangles, 12);
        assert!(s.closed && !s.oriented);
        assert_eq!(s.volume, -8000.0);
    }

    #[test]
    fn an_embedded_part_and_an_inward_cavity_are_not_inverted() {
        let embedded = concat(&[&cube(), &make_box([5.0; 3], [10.0; 3])]).unwrap();
        let s = status(&embedded);
        assert_eq!((s.inverted_triangles, s.shells), (0, 2));
        assert!(s.closed && s.oriented);
        let hollow = concat(&[&cube(), &inside_out(make_box([5.0; 3], [10.0; 3]))]).unwrap();
        let d = detail(&hollow);
        assert_eq!(d.status.inverted_triangles, 0);
        assert!(d.inverted.is_empty() && d.status.oriented);
        assert_eq!(d.status.volume, 8000.0 - 125.0);
    }

    #[test]
    fn interpenetrating_cubes_are_two_outward_shells() {
        let two = concat(&[&cube(), &make_box([10.0, 0.0, 0.0], [30.0, 20.0, 20.0])]).unwrap();
        let s = status(&two);
        assert_eq!((s.inverted_triangles, s.shells, s.holes), (0, 2, 0));
        assert!(s.closed && s.oriented);
    }

    #[test]
    fn three_fins_on_an_edge_are_non_manifold() {
        let m = Mesh {
            positions: vec![
                0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 0.0,
            ],
            indices: vec![0, 1, 2, 0, 1, 3, 0, 1, 4],
        };
        let d = detail(&m);
        let s = &d.status;
        assert_eq!((s.non_manifold_edges, s.border_edges, s.edges), (1, 6, 7));
        assert!(!s.closed && s.oriented);
        assert_eq!(d.non_manifold, vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(s.inverted_triangles, 0);
    }

    #[test]
    fn a_mobius_strip_is_not_oriented() {
        let n = 8u32;
        let mut positions = Vec::new();
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let (c, s) = ((a / 2.0).cos(), (a / 2.0).sin());
            for w in [1.0f32, -1.0] {
                let r = 4.0 + w * c;
                positions.extend_from_slice(&[r * a.cos(), r * a.sin(), w * s]);
            }
        }
        let mut indices = Vec::new();
        for i in 0..n {
            let (t, b) = (2 * i, 2 * i + 1);
            let (nt, nb) = if i + 1 < n {
                (2 * i + 2, 2 * i + 3)
            } else {
                (1, 0)
            };
            indices.extend_from_slice(&[t, b, nb, t, nb, nt]);
        }
        let s = status(&Mesh { positions, indices });
        assert!(!s.oriented && !s.closed);
        assert_eq!(s.holes, 1);
    }

    #[test]
    fn degenerate_triangles_by_shortest_edge_and_zero_area() {
        let m = Mesh {
            positions: vec![
                0.0, 0.0, 0.0, // 0
                10.0, 0.0, 0.0, // 1
                0.0, 10.0, 0.0, // 2
                5.0, 0.001, 0.0, // 3: 0.001 above the edge 0-1
                10.0, 0.0005, 0.0, // 4: 0.0005 from vertex 1
                5.0, 0.0, 0.0, // 5: on the edge 0-1
                0.0, 0.0, 0.0, // 6: same place as vertex 0
            ],
            indices: vec![
                0, 1, 2, // fine
                0, 1, 3, // thin, height 0.001, but every edge long: not degenerate
                1, 2, 4, // needle: shortest edge 0.0005
                0, 1, 5, // zero area, all edges long
                0, 6, 1, // zero-length edge between distinct vertices
                0, 0, 1, // repeated index
            ],
        };
        assert_eq!(degenerate_triangles(&m, 0.002), vec![2, 3, 4, 5]);
        assert_eq!(degenerate_triangles(&m, 0.0008), vec![2, 3, 4, 5]);
        assert_eq!(degenerate_triangles(&m, 0.0001), vec![3, 4, 5]);
        // Even with no tolerance at all, zero area is found exactly.
        assert_eq!(degenerate_triangles(&m, 0.0), vec![3, 4, 5]);
        assert!(degenerate_triangles(&cube(), 0.002).is_empty());
    }

    #[test]
    fn the_status_counts_needles_but_not_thin_triangles() {
        // The top face's triangle (4, 5, 6) is split round a new vertex 8: (4, 5, 8), (5, 6, 8)
        // and (6, 4, 8).
        let split = |p: [f32; 3]| {
            let mut m = cube();
            m.positions.extend_from_slice(&p);
            m.indices[6..9].copy_from_slice(&[4, 5, 8]);
            m.indices.extend_from_slice(&[5, 6, 8, 6, 4, 8]);
            m
        };
        // 0.0005 from the long edge 4-6: (6, 4, 8) is that thin, but its corners lie far apart.
        let d = 0.0005 * std::f32::consts::FRAC_1_SQRT_2;
        let thin = split([10.0 + d, 10.0 - d, 20.0]);
        let s = status(&thin);
        assert_eq!(s.degenerate_triangles, 0);
        assert!(s.closed && s.oriented);
        assert!((s.volume - 8000.0).abs() < 1e-9);
        // 0.001 from corner 6: both triangles with the edge 6-8 are needles.
        let needles = split([19.9995, 19.9991, 20.0]);
        assert_eq!(effective_degenerate_tolerance(&DEGENERATE), 0.002);
        let d = detail(&needles);
        assert_eq!(d.degenerate, vec![12, 13]);
        assert_eq!(d.status.degenerate_triangles, 2);
        assert!(d.status.closed && d.status.oriented);
    }

    #[test]
    fn progress_rises_and_cancel_stops() {
        let m = cube();
        let never = std::sync::atomic::AtomicBool::new(false);
        let mut seen = Vec::new();
        {
            let mut control = Control::new(&never, |f| seen.push(f));
            analyze(&m, &mut control).unwrap();
        }
        assert!(seen.is_sorted() && seen.last() == Some(&1.0));
        let stop = std::sync::atomic::AtomicBool::new(true);
        let mut control = Control::new(&stop, |_| {});
        assert_eq!(
            analyze(&m, &mut control),
            Err(crate::repair::RepairError::Cancelled)
        );
    }

    /// A closed torus of `n` × `n` quads (2n² triangles): many triangles, one closed shell.
    fn torus(n: u32) -> Mesh {
        let mut positions = Vec::with_capacity((n * n * 3) as usize);
        for i in 0..n {
            let u = i as f32 / n as f32 * std::f32::consts::TAU;
            for j in 0..n {
                let v = j as f32 / n as f32 * std::f32::consts::TAU;
                let r = 40.0 + 15.0 * v.cos();
                positions.extend_from_slice(&[r * u.cos(), r * u.sin(), 15.0 * v.sin()]);
            }
        }
        let mut indices = Vec::with_capacity((n * n * 6) as usize);
        for i in 0..n {
            for j in 0..n {
                let a = i * n + j;
                let b = (i + 1) % n * n + j;
                let c = (i + 1) % n * n + (j + 1) % n;
                let d = i * n + (j + 1) % n;
                indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
        Mesh { positions, indices }
    }

    #[test]
    fn a_torus_is_closed_and_oriented() {
        let m = torus(24);
        let s = status(&m);
        assert!(s.closed && s.oriented && s.volume > 0.0);
        assert_eq!((s.shells, s.holes, s.edges), (1, 0, 3 * 24 * 24));
        assert_eq!(s.inverted_triangles, 0);
    }

    /// Run with `cargo test --release --lib repair::status -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn analyze_a_million_triangles() {
        // A 708 × 708 torus (1,002,528 triangles) with holes, turned triangles and a second shell.
        let mut m = torus(708);
        for t in (0..m.triangle_count()).step_by(997) {
            flip(&mut m, t);
        }
        let mut keep = vec![true; m.triangle_count()];
        for t in (500..m.triangle_count()).step_by(50_021) {
            keep[t] = false;
        }
        let (mut m, _) = crate::repair::topology::keep_triangles(&m, &keep);
        m = concat(&[&m, &make_box([-5.0; 3], [5.0; 3])]).unwrap();
        let started = Instant::now();
        let d = silent(|control| analyze(&m, control)).unwrap();
        let took = started.elapsed().as_secs_f64();
        println!(
            "analyze: {} triangles in {:.3} s: {:?}",
            m.triangle_count(),
            took,
            d.status
        );
        assert_eq!(d.status.holes, 21);
        assert_eq!(d.status.shells, 2);
        assert!(d.status.inverted_triangles > 900 && !d.status.oriented);
        if !cfg!(debug_assertions) {
            assert!(took < 1.0, "analyze took {took:.3} s");
        }
    }

    /// The status of real files, with timings: `TRICKY3D_MODELS="a.stl;b.3mf" cargo test
    /// --release --lib repair::status::tests::real_models -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_models() {
        use crate::repair::topology::{boundary_loops, components, orientation_patches};
        let Ok(list) = std::env::var("TRICKY3D_MODELS") else {
            return;
        };
        for path in list.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let started = Instant::now();
            let imported = crate::formats::import_file(
                std::path::Path::new(path),
                true,
                &mut crate::progress::Progress::silent(),
            )
            .unwrap();
            println!("{path}: read in {:.2} s", started.elapsed().as_secs_f64());
            for part in &imported.parts {
                let m = &part.mesh;
                let started = Instant::now();
                let d = silent(|control| analyze(m, control)).unwrap();
                let took = started.elapsed().as_secs_f64();
                agrees_with_the_info_panel(m, &d);
                // The same steps one by one, for the breakdown.
                let mut lap = Instant::now();
                let mut split = |name: &str| {
                    print!("{name} {:.3}  ", lap.elapsed().as_secs_f64());
                    lap = Instant::now();
                };
                let edges = Edges::new(m);
                split("edges");
                let loops = boundary_loops(m, &edges);
                split("loops");
                let _ = components(m, &edges, Link::Shell);
                split("shells");
                let patches = orientation_patches(m, &edges);
                split("patches");
                let flipped = silent(|control| {
                    orient::inverted(m, &edges, &patches, orient::Nested::AsMost, control)
                })
                .unwrap();
                split("inverted");
                let tolerance = effective_degenerate_tolerance(&DEGENERATE);
                let degenerate = degenerate_triangles(m, tolerance);
                split("degenerate");
                let (rows, _) = crate::repair::shells::shell_table(m);
                split("shell table");
                println!();
                assert_eq!(loops.len() as u64, d.status.holes);
                assert_eq!(
                    flipped.iter().filter(|&&f| f).count() as u64,
                    d.status.inverted_triangles
                );
                assert_eq!(degenerate, d.degenerate);
                assert_eq!(rows.len() as u64, d.status.shells);
                let closed_patches = patches.closed.iter().filter(|&&c| c).count();
                println!(
                    "  analyze {took:.3} s; {} patches ({closed_patches} closed, {} conflicts); \
                     {} watertight shells; diagonal {:.1} mm, degenerate below {tolerance:.5} mm; \
                     {:?}",
                    patches.count,
                    patches.conflicts,
                    rows.iter().filter(|r| r.watertight).count(),
                    bbox_diagonal(m),
                    d.status
                );
            }
        }
    }
}
