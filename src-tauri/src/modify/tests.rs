use std::sync::atomic::AtomicBool;

use serde_json::json;

use super::reduce::ReduceSettings;
use super::refine::RefineSettings;
use super::smooth::SmoothSettings;
use super::*;
use crate::analysis::compare::{CompareSettings, Direction, compare};
use crate::arrange::World;
use crate::mesh::{IDENTITY, make_box, stats};
use crate::repair::exact::testing::Rng;
use crate::repair::{Control, silent};

pub(super) fn sphere(r: f64, segments: u32, rings: u32) -> Mesh {
    let params = json!({"radius": r, "segments": segments, "rings": rings});
    crate::library::generate("sphere", params.as_object().unwrap()).unwrap()
}

pub(super) fn run_tool(tool: &Tool, mesh: &Mesh, selection: &[u32]) -> RepairResult<Modified> {
    silent(|c| run(tool, mesh, selection, c))
}

/// The largest distance either way between two surfaces, by mesh compare.
pub(super) fn deviation(a: &Mesh, b: &Mesh) -> f64 {
    let s = CompareSettings {
        direction: Direction::Both,
        align: false,
    };
    let c =
        silent(|c| compare(&World::new(a, &IDENTITY), &World::new(b, &IDENTITY), &s, c)).unwrap();
    let f = c.forward.unwrap().0.max_unsigned;
    let g = c.backward.unwrap().0.max_unsigned;
    f.max(g)
}

pub(super) fn assert_sound(mesh: &Mesh) {
    let s = silent(|c| soundness(mesh, c)).unwrap();
    assert!(s.closed && s.oriented && s.crossings == 0, "{s:?}");
}

fn reduce(target: u64, lowest: bool, max_deviation: f64, max_edge: f64, fast: bool) -> Tool {
    Tool::Reduce(ReduceSettings {
        target,
        lowest,
        max_deviation,
        max_edge,
        selected_only: false,
        fast,
    })
}

/// A box with every face split into a fine grid (each triangle into four, `times` times).
fn fine_box(times: usize) -> Mesh {
    let mut mesh = make_box([0.0; 3], [20.0; 3]);
    for _ in 0..times {
        mesh = silent(|c| crate::repair::refine::subdivide(&mesh, &[], c))
            .unwrap()
            .mesh;
    }
    mesh
}

#[test]
fn a_sphere_reduced_to_a_tenth_stays_within_the_max_deformation() {
    // 2 × 160 × 80 = 25,280 triangles, radius 20 mm.
    let ball = sphere(20.0, 160, 80);
    let n = ball.triangle_count();
    for fast in [false, true] {
        let tool = reduce((n / 10) as u64, false, 0.05, 100.0, fast);
        let out = run_tool(&tool, &ball, &[]).unwrap();
        assert!(out.changed);
        let left = out.mesh.triangle_count();
        assert!(left <= n / 10, "fast {fast}: {left}");
        assert!(left >= n / 10 - 2, "fast {fast}: {left}");
        assert_sound(&out.mesh);
        let d = deviation(&ball, &out.mesh);
        assert!(d <= 0.05, "fast {fast}: deviation {d}");
        // Every surface point within 0.05 mm: the volume within area × 0.05 = 1.5 % of it here.
        let (v0, v1) = (stats(&ball).volume, stats(&out.mesh).volume);
        let area = stats(&ball).area;
        assert!((v1 - v0).abs() <= area * 0.05, "fast {fast}: {v0} → {v1}");
        // The origin map names the surviving input triangles, in order.
        let origin = out.origin.unwrap();
        assert_eq!(origin.len(), left);
        assert!(origin.windows(2).all(|w| w[0] < w[1]));
    }
}

#[test]
fn a_fine_box_goes_down_to_twelve_triangles_as_low_as_possible() {
    let mesh = fine_box(3);
    assert_eq!(mesh.triangle_count(), 12 * 64);
    let out = run_tool(&reduce(0, true, 0.01, 100.0, false), &mesh, &[]).unwrap();
    assert_eq!(out.mesh.triangle_count(), 12, "{}", out.message);
    assert_sound(&out.mesh);
    assert!((stats(&out.mesh).volume - 8000.0).abs() < 1e-3);
    // Every corner of the result is a corner of the box.
    for p in out.mesh.positions.as_chunks::<3>().0 {
        assert!(p.iter().all(|&x| x == 0.0 || x == 20.0), "{p:?}");
    }
}

#[test]
fn max_edge_length_and_the_selection_hold_the_reduction_back() {
    let mesh = fine_box(3);
    // Edges of 20 / 8 = 2.5 mm (diagonals 3.5 mm); none may grow past 6 mm.
    let out = run_tool(&reduce(0, true, 0.01, 6.0, false), &mesh, &[]).unwrap();
    assert!(out.mesh.triangle_count() > 12);
    for t in out.mesh.indices.as_chunks::<3>().0 {
        for k in 0..3 {
            let (a, b) = (out.mesh.vertex(t[k]), out.mesh.vertex(t[(k + 1) % 3]));
            let d: f64 = (0..3).map(|i| (a[i] as f64 - b[i] as f64).powi(2)).sum();
            assert!(d.sqrt() <= 6.0 + 1e-9);
        }
    }
    // Only the top face selected (z = 20): every other triangle stays exactly as it was.
    let top: Vec<u32> = (0..mesh.triangle_count() as u32)
        .filter(|&t| {
            mesh.indices[3 * t as usize..3 * t as usize + 3]
                .iter()
                .all(|&v| mesh.vertex(v)[2] == 20.0)
        })
        .collect();
    let mut tool = reduce(0, true, 0.01, 100.0, false);
    if let Tool::Reduce(s) = &mut tool {
        s.selected_only = true;
    }
    let out = run_tool(&tool, &mesh, &top).unwrap();
    assert_sound(&out.mesh);
    let kept = |m: &Mesh| -> Vec<[[f32; 3]; 3]> {
        let mut v: Vec<_> = m
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| t.map(|i| m.vertex(i)))
            .filter(|c| c.iter().any(|p| p[2] < 20.0))
            .collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v
    };
    assert_eq!(kept(&out.mesh), kept(&mesh));
    assert!(out.mesh.triangle_count() < mesh.triangle_count());
    // Selected triangles only with nothing selected says so.
    assert!(run_tool(&tool, &mesh, &[]).is_err());
}

#[test]
fn a_noisy_sphere_smooths_without_shrinking() {
    // A cube sphere (a fine box blown up onto the sphere: no slivers at poles), radius 20 mm, each
    // point moved in or out by up to 0.3 mm.
    let r = 20.0;
    let mut ball = fine_box(4);
    let mut rng = Rng(7);
    for p in ball.positions.as_chunks_mut::<3>().0 {
        for x in p.iter_mut() {
            *x -= 10.0;
        }
        let len = (p.iter().map(|&x| (x as f64).powi(2)).sum::<f64>()).sqrt();
        let k = (r + (rng.unit() - 0.5) * 0.6) / len;
        for x in p.iter_mut() {
            *x = (*x as f64 * k) as f32;
        }
    }
    let radii = |m: &Mesh| -> (f64, f64) {
        let rs: Vec<f64> = m
            .positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| (p.iter().map(|&x| (x as f64).powi(2)).sum::<f64>()).sqrt())
            .collect();
        let mean = rs.iter().sum::<f64>() / rs.len() as f64;
        let dev = (rs.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / rs.len() as f64).sqrt();
        (mean, dev)
    };
    let (_, rough0) = radii(&ball);
    let tool = Tool::Smooth(SmoothSettings {
        strength: 0.5,
        iterations: 10,
        keep_edges: false,
        edge_angle: 60.0,
        selected_only: false,
        keep_volume: true,
    });
    let out = run_tool(&tool, &ball, &[]).unwrap();
    assert_sound(&out.mesh);
    let (mean, rough) = radii(&out.mesh);
    assert!((mean - r).abs() < 0.01 * r, "mean radius {mean}");
    assert!(rough < 0.5 * rough0, "roughness {rough0} → {rough}");
    let (v0, v1) = (stats(&ball).volume, stats(&out.mesh).volume);
    assert!((v1 - v0).abs() < 1e-4 * v0, "volume {v0} → {v1}");
    // Without keeping the volume Taubin's steps still shrink it little.
    let mut plain = tool.clone();
    if let Tool::Smooth(s) = &mut plain {
        s.keep_volume = false;
    }
    let out = run_tool(&plain, &ball, &[]).unwrap();
    let (mean, _) = radii(&out.mesh);
    assert!((mean - r).abs() < 0.01 * r, "mean radius {mean}");
}

#[test]
fn smoothing_keeps_a_box_sharp_and_leaves_unselected_points() {
    let mesh = fine_box(2);
    let tool = |keep: bool| {
        Tool::Smooth(SmoothSettings {
            strength: 0.6,
            iterations: 5,
            keep_edges: keep,
            edge_angle: 30.0,
            selected_only: false,
            keep_volume: false,
        })
    };
    // With sharp edges kept, every point stays on the box's surface.
    let out = run_tool(&tool(true), &mesh, &[]).unwrap();
    for p in out.mesh.positions.as_chunks::<3>().0 {
        assert!(
            p.iter().any(|&x| x == 0.0 || x == 20.0),
            "{p:?} left the box's faces"
        );
        assert!(p.iter().all(|&x| (0.0..=20.0).contains(&x)));
    }
    // Without, the corners round off: no point is left at the corner (0, 0, 0).
    let out = run_tool(&tool(false), &mesh, &[]).unwrap();
    let nearest = out
        .mesh
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| p.iter().map(|&x| (x as f64).powi(2)).sum::<f64>().sqrt())
        .fold(f64::INFINITY, f64::min);
    assert!(nearest > 0.5, "{nearest}");
    // Selected only: the bottom face's triangles only; no point off it moves.
    let bottom: Vec<u32> = (0..mesh.triangle_count() as u32)
        .filter(|&t| {
            mesh.indices[3 * t as usize..3 * t as usize + 3]
                .iter()
                .all(|&v| mesh.vertex(v)[2] == 0.0)
        })
        .collect();
    let mut only = tool(false);
    if let Tool::Smooth(s) = &mut only {
        s.selected_only = true;
    }
    let out = run_tool(&only, &mesh, &bottom).unwrap();
    for (a, b) in mesh
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .zip(out.mesh.positions.as_chunks::<3>().0)
    {
        if a[2] != 0.0 || a[0] == 0.0 || a[0] == 20.0 || a[1] == 0.0 || a[1] == 20.0 {
            assert_eq!(a, b);
        }
    }
}

#[test]
fn refining_keeps_every_point_on_the_surface_and_every_edge_under_the_limit() {
    let ball = sphere(10.0, 24, 12);
    let tool = Tool::Refine(RefineSettings {
        max_edge: 0.8,
        selected_only: false,
    });
    let out = run_tool(&tool, &ball, &[]).unwrap();
    assert_sound(&out.mesh);
    assert!(out.mesh.triangle_count() > 4 * ball.triangle_count());
    for t in out.mesh.indices.as_chunks::<3>().0 {
        for k in 0..3 {
            let (a, b) = (out.mesh.vertex(t[k]), out.mesh.vertex(t[(k + 1) % 3]));
            let d: f64 = (0..3).map(|i| (a[i] as f64 - b[i] as f64).powi(2)).sum();
            assert!(d.sqrt() <= 0.8, "{}", d.sqrt());
        }
    }
    // Every point lies on the original surface (to f32 rounding).
    let surface = crate::ops::shell::field::Surface::new(&ball);
    for v in 0..out.mesh.vertex_count() as u32 {
        let p = crate::repair::exact::vertex(&out.mesh, v);
        let d = surface.distance(p, 1.0);
        assert!(d < 1e-5, "point {v} is {d} mm off");
    }
    assert!((stats(&out.mesh).volume - stats(&ball).volume).abs() < 1e-2);
    // Selected only: only the selected triangle and its neighbours along shared edges split.
    let sel = Tool::Refine(RefineSettings {
        max_edge: 0.8,
        selected_only: true,
    });
    let out = run_tool(&sel, &ball, &[100]).unwrap();
    assert_sound(&out.mesh);
    let origin = out.origin.unwrap();
    let from_100 = origin.iter().filter(|&&t| t == 100).count();
    assert!(from_100 > 4);
    let untouched = origin.iter().filter(|&&t| t != 100).count();
    assert!(untouched < ball.triangle_count() + 40);
}

#[test]
fn inverting_turns_the_part_inside_out_and_repair_flags_it() {
    let cube = make_box([0.0; 3], [20.0; 3]);
    let out = run_tool(&Tool::Invert, &cube, &[]).unwrap();
    assert!(out.changed);
    assert_eq!(stats(&out.mesh).volume, -8000.0);
    let s = silent(|c| crate::repair::status::analyze(&out.mesh, c))
        .unwrap()
        .status;
    assert_eq!(s.inverted_triangles, 12);
    assert!(!s.oriented);
    // Invert negative shells turns it back.
    let back = run_tool(&Tool::InvertNegative, &out.mesh, &[]).unwrap();
    assert_eq!(stats(&back.mesh).volume, 8000.0);
    assert_eq!(back.mesh, cube);
}

#[test]
fn a_result_that_would_break_the_part_is_refused() {
    let sound = Soundness {
        closed: true,
        oriented: true,
        consistent: true,
        crossings: 0,
    };
    let crossing = Soundness {
        crossings: 3,
        ..sound
    };
    let tool = reduce(10, false, 0.1, 10.0, false);
    let why = refusal(&tool, &sound, &crossing).unwrap();
    assert!(
        why.starts_with("Triangle reduction would make triangles cross each other"),
        "{why}"
    );
    // A part that already had crossings may keep them.
    assert_eq!(refusal(&tool, &crossing, &crossing), None);
    let open = Soundness {
        closed: false,
        ..sound
    };
    assert!(refusal(&tool, &sound, &open).unwrap().contains("open it"));
    // Invert leaves the part inside out on purpose.
    let inside_out = Soundness {
        oriented: false,
        ..sound
    };
    assert_eq!(refusal(&Tool::Invert, &sound, &inside_out), None);
    assert!(refusal(&tool, &sound, &inside_out).is_some());
}

#[test]
fn bad_settings_and_a_cancel_stop_before_anything_changes() {
    let ball = sphere(10.0, 32, 16);
    assert!(run_tool(&reduce(100, false, 0.0, 10.0, false), &ball, &[]).is_err());
    assert!(run_tool(&reduce(2, false, 0.1, 10.0, false), &ball, &[]).is_err());
    let cancel = AtomicBool::new(true);
    let mut control = Control::new(&cancel, |_| {});
    assert_eq!(
        run(
            &reduce(100, false, 0.1, 10.0, false),
            &ball,
            &[],
            &mut control
        ),
        Err(RepairError::Cancelled)
    );
}

// ---------------------------------------------------------------------------------------------
// Part C: remesh, Z-compensation, extrude and offsets

use super::extrude::{Direction as Way, ExtrudeSettings, HoleOffsetSettings, OffsetSettings};
use super::remesh::{Method, RemeshSettings, smallest_angle};
use super::zcomp::ZCompSettings;

fn remesh_tool(method: Method, edge: f64) -> Tool {
    Tool::Remesh(RemeshSettings {
        method,
        edge,
        large_edge: 4.0 * edge,
        smoothing: 0.0,
        keep_edges: true,
        edge_angle: 45.0,
        fast: false,
        selected_only: false,
    })
}

/// Mean, shortest and longest edge.
fn edge_lengths(mesh: &Mesh) -> (f64, f64, f64) {
    let mut sum = 0.0;
    let mut count = 0;
    let (mut lo, mut hi) = (f64::INFINITY, 0.0f64);
    for t in mesh.indices.as_chunks::<3>().0 {
        for k in 0..3 {
            let (a, b) = (mesh.vertex(t[k]), mesh.vertex(t[(k + 1) % 3]));
            let d = (0..3)
                .map(|i| (a[i] as f64 - b[i] as f64).powi(2))
                .sum::<f64>()
                .sqrt();
            sum += d;
            count += 1;
            lo = lo.min(d);
            hi = hi.max(d);
        }
    }
    (sum / count as f64, lo, hi)
}

fn slim_count(mesh: &Mesh, degrees: f64) -> usize {
    (0..mesh.triangle_count() as u32)
        .filter(|&t| smallest_angle(&crate::repair::exact::corners(mesh, t)) < degrees)
        .count()
}

#[test]
fn a_sphere_remeshed_by_each_method_keeps_its_shape_and_its_target_edges() {
    let ball = sphere(10.0, 96, 48);
    let edge = 1.0;
    for (method, fast, near, off) in [
        (Method::Volumetric, false, (0.75, 1.25), 0.3),
        // Fast mode leaves each point halfway along its grid edge: up to most of a step off.
        (Method::Volumetric, true, (0.75, 1.25), 1.6 * edge),
        (Method::Uniform, false, (0.8, 1.2), 0.05),
        // Manifold lies half an edge length outside, by design.
        (Method::Manifold, false, (0.75, 1.25), 0.5 * edge + 0.3),
    ] {
        let mut tool = remesh_tool(method, edge);
        if let Tool::Remesh(s) = &mut tool {
            s.fast = fast;
        }
        let out = run_tool(&tool, &ball, &[]).unwrap();
        assert_sound(&out.mesh);
        let (mean, _, _) = edge_lengths(&out.mesh);
        assert!(
            (near.0..=near.1).contains(&(mean / edge)),
            "{method:?} fast {fast}: mean edge {mean}"
        );
        let d = deviation(&ball, &out.mesh);
        let (_, lo, hi) = edge_lengths(&out.mesh);
        println!(
            "{method:?} fast {fast}: {} triangles, edges {lo:.3}..{hi:.3} mean {mean:.3}, deviation {d:.4}, smallest angle {:.1}°",
            out.mesh.triangle_count(),
            (0..out.mesh.triangle_count() as u32)
                .map(|t| smallest_angle(&crate::repair::exact::corners(&out.mesh, t)))
                .fold(f64::INFINITY, f64::min)
        );
        assert!(d <= off, "{method:?} fast {fast}: deviation {d}");
        if method == Method::Uniform {
            assert_eq!(slim_count(&out.mesh, 10.0), 0, "slim triangles under 10°");
        }
    }
    // Adaptive keeps between its small and large edge lengths.
    let mut tool = remesh_tool(Method::Adaptive, 0.5);
    if let Tool::Remesh(s) = &mut tool {
        s.large_edge = 3.0;
    }
    let out = run_tool(&tool, &ball, &[]).unwrap();
    assert_sound(&out.mesh);
    let (mean, _, _) = edge_lengths(&out.mesh);
    assert!(
        (0.5 * 0.8..=3.0 * 4.0 / 3.0).contains(&mean),
        "adaptive mean edge {mean}"
    );
    let d = deviation(&ball, &out.mesh);
    println!(
        "Adaptive 0.5..3: {} triangles, mean edge {mean:.3}, deviation {d:.4}",
        out.mesh.triangle_count()
    );
    assert!(d <= 0.1);
}

#[test]
fn uniform_remeshing_keeps_a_box_sharp_and_volumetric_needs_a_closed_part() {
    let mesh = make_box([0.0; 3], [20.0; 3]);
    let out = run_tool(&remesh_tool(Method::Uniform, 2.0), &mesh, &[]).unwrap();
    assert_sound(&out.mesh);
    assert!(out.mesh.triangle_count() > 500);
    // Every point stays on the faces of the box, so the volume is the same.
    for p in out.mesh.positions.as_chunks::<3>().0 {
        assert!(
            p.iter().any(|&x| x.abs() < 1e-4 || (x - 20.0).abs() < 1e-4),
            "{p:?}"
        );
    }
    assert!((stats(&out.mesh).volume - 8000.0).abs() < 1e-2);
    // An open part: Volumetric refuses, Manifold closes it with a skin.
    let mut open = mesh.clone();
    open.indices.truncate(open.indices.len() - 3);
    let err = run_tool(&remesh_tool(Method::Volumetric, 2.0), &open, &[]).unwrap_err();
    assert!(
        matches!(err, RepairError::Failed(ref s) if s.starts_with("Volumetric remeshing needs a closed part"))
    );
    let skin = run_tool(&remesh_tool(Method::Manifold, 2.0), &open, &[]).unwrap();
    assert_sound(&skin.mesh);
}

#[test]
fn z_compensation_raises_only_the_bottom_of_a_box() {
    let mesh = fine_box(2);
    let tool = Tool::ZCompensation(ZCompSettings {
        distance: 0.3,
        angle: 10.0,
    });
    let out = run_tool(&tool, &mesh, &[]).unwrap();
    assert_sound(&out.mesh);
    assert_eq!(out.mesh.indices, mesh.indices);
    for (a, b) in mesh
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .zip(out.mesh.positions.as_chunks::<3>().0)
    {
        if a[2] == 0.0 {
            assert_eq!(b, &[a[0], a[1], 0.3]);
        } else {
            assert_eq!(a, b);
        }
    }
    assert!((stats(&out.mesh).volume - 400.0 * 19.7).abs() < 1e-2);
    // Past the height of the box the bottom would cross the top: refused.
    let far = Tool::ZCompensation(ZCompSettings {
        distance: 10.0,
        angle: 10.0,
    });
    let tall = make_box([0.0; 3], [20.0, 20.0, 5.0]);
    assert!(run_tool(&far, &tall, &[]).is_err());
}

/// The triangles of `mesh` whose corners all lie at z = `z`.
fn face_at(mesh: &Mesh, z: f32) -> Vec<u32> {
    (0..mesh.triangle_count() as u32)
        .filter(|&t| {
            mesh.indices[3 * t as usize..3 * t as usize + 3]
                .iter()
                .all(|&v| mesh.vertex(v)[2] == z)
        })
        .collect()
}

#[test]
fn extruding_the_top_of_a_cube_by_5_mm_gives_a_20_by_20_by_25_box() {
    let cube = make_box([0.0; 3], [20.0; 3]);
    let top = face_at(&cube, 20.0);
    assert_eq!(top.len(), 2);
    for walls in [false, true] {
        for direction in [Way::Normal, Way::Z] {
            let tool = Tool::Extrude(ExtrudeSettings {
                distance: 5.0,
                direction,
                walls,
            });
            let out = run_tool(&tool, &cube, &top).unwrap();
            assert_sound(&out.mesh);
            let b = crate::mesh::bounds(&out.mesh).unwrap();
            assert_eq!(b, [[0.0, 0.0, 0.0], [20.0, 20.0, 25.0]], "walls {walls}");
            assert!((stats(&out.mesh).volume - 10_000.0).abs() < 1e-6);
            assert_eq!(out.mesh.triangle_count(), if walls { 20 } else { 12 });
        }
    }
    // Pushed in past the bottom, the top would cross it: refused. Nothing selected: said so.
    let deep = Tool::Extrude(ExtrudeSettings {
        distance: -25.0,
        direction: Way::Normal,
        walls: false,
    });
    assert!(run_tool(&deep, &cube, &top).is_err());
    assert!(run_tool(&deep, &cube, &[]).is_err());
}

#[test]
fn offsetting_a_selection_moves_only_its_points() {
    let mesh = fine_box(2);
    let top = face_at(&mesh, 20.0);
    let tool = Tool::OffsetSelection(OffsetSettings {
        distance: 1.0,
        transition: 0.0,
    });
    let out = run_tool(&tool, &mesh, &top).unwrap();
    assert_sound(&out.mesh);
    let mut moved = 0;
    for (a, b) in mesh
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .zip(out.mesh.positions.as_chunks::<3>().0)
    {
        if a[2] == 20.0 {
            assert_eq!(b, &[a[0], a[1], 21.0]);
            moved += 1;
        } else {
            assert_eq!(a, b);
        }
    }
    assert_eq!(moved, 25);
    // With a 6 mm transition the row 5 mm below the top follows part of the way, 10 mm not.
    let eased = Tool::OffsetSelection(OffsetSettings {
        distance: 1.0,
        transition: 6.0,
    });
    let out = run_tool(&eased, &mesh, &top).unwrap();
    assert_sound(&out.mesh);
    let shift = |z: f32| -> f64 {
        mesh.positions
            .as_chunks::<3>()
            .0
            .iter()
            .zip(out.mesh.positions.as_chunks::<3>().0)
            .filter(|(a, _)| a[2] == z && a[0] == 0.0 && a[1] == 10.0)
            .map(|(a, b)| {
                (0..3)
                    .map(|k| (b[k] as f64 - a[k] as f64).powi(2))
                    .sum::<f64>()
                    .sqrt()
            })
            .next()
            .unwrap()
    };
    let (one, two) = (shift(15.0), shift(10.0));
    assert!(one > 0.0 && one < 1.0 && two == 0.0, "{one} {two}");
}

/// A tube without ends: `n` steps round, radius `r`, from z = 0 to `h`, facing out.
fn open_tube(n: u32, r: f64, h: f64) -> Mesh {
    let mut positions = Vec::new();
    for z in [0.0, h] {
        for i in 0..n {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            positions.extend_from_slice(&[(r * a.cos()) as f32, (r * a.sin()) as f32, z as f32]);
        }
    }
    let mut indices = Vec::new();
    for i in 0..n {
        let j = (i + 1) % n;
        let (b0, b1, t0, t1) = (i, j, n + i, n + j);
        indices.extend_from_slice(&[b0, b1, t1, b0, t1, t0]);
    }
    Mesh { positions, indices }
}

#[test]
fn offsetting_hole_edges_adds_a_ring_of_the_given_width() {
    let tube = open_tube(48, 10.0, 20.0);
    let tool = Tool::OffsetHoles(HoleOffsetSettings {
        distance: 2.5,
        selected_only: false,
    });
    let out = run_tool(&tool, &tube, &[]).unwrap();
    assert_eq!(out.mesh.triangle_count(), tube.triangle_count() + 4 * 48);
    // The new points: on the wall of the tube, 2.5 mm past each end.
    for p in out.mesh.positions.as_chunks::<3>().0[tube.vertex_count()..].iter() {
        let radius = ((p[0] as f64).powi(2) + (p[1] as f64).powi(2)).sqrt();
        assert!((radius - 10.0).abs() < 1e-3, "{radius}");
        assert!(
            (p[2] as f64 + 2.5).abs() < 1e-4 || (p[2] as f64 - 22.5).abs() < 1e-4,
            "{p:?}"
        );
    }
    let s = silent(|c| crate::repair::status::analyze(&out.mesh, c))
        .unwrap()
        .status;
    assert_eq!(s.holes, 2);
    assert!(s.oriented);
}

#[test]
fn a_thin_plate_smooths_and_remeshes_without_crossing_itself() {
    // 20 x 20 x 0.3 mm, each face split into a fine grid: the two sides are 0.3 mm apart, much
    // nearer than the triangles are long.
    let mut plate = make_box([0.0; 3], [20.0, 20.0, 0.3]);
    for _ in 0..3 {
        plate = silent(|c| crate::repair::refine::subdivide(&plate, &[], c))
            .unwrap()
            .mesh;
    }
    let smooth = Tool::Smooth(SmoothSettings {
        strength: 1.0,
        iterations: 20,
        keep_edges: false,
        edge_angle: 60.0,
        selected_only: false,
        keep_volume: true,
    });
    let out = run_tool(&smooth, &plate, &[]).unwrap();
    assert_sound(&out.mesh);
    let uniform = remesh_tool(Method::Uniform, 1.0);
    let out = run_tool(&uniform, &plate, &[]).unwrap();
    assert_sound(&out.mesh);
    let d = deviation(&plate, &out.mesh);
    println!(
        "thin plate remeshed: {} triangles, deviation {d:.3}",
        out.mesh.triangle_count()
    );
    assert!(d < 0.05, "{d}");
}
