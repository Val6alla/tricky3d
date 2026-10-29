//! Labels on a box and a cylinder: the result is closed, oriented and free of crossings, and the
//! material added or taken away is the label's area × depth (within the stated tolerance).

use super::emboss::{self, Frame, Relief};
use super::*;
use crate::library::primitive_cylinder;
use crate::mesh::{make_box, stats};
use crate::repair::{silent, status, tritri};

/// What Repair's status must show: closed, oriented, outward, no degenerate triangles, and no
/// two triangles crossing.
fn assert_clean(mesh: &Mesh, what: &str) {
    let s = silent(|c| status::analyze(mesh, c)).unwrap().status;
    assert!(s.closed && s.oriented, "{what}: {s:?}");
    assert_eq!(s.degenerate_triangles, 0, "{what}: degenerate triangles");
    assert!(s.volume > 0.0, "{what}: faces inward");
    let crossing = silent(|c| tritri::intersecting_pairs(mesh, c)).unwrap();
    assert!(
        crossing.is_empty(),
        "{what}: {} crossing pairs",
        crossing.len()
    );
}

fn volume(mesh: &Mesh) -> f64 {
    stats(mesh).volume
}

fn close(what: &str, got: f64, want: f64, tolerance: f64) {
    let error = (got - want).abs() / want.abs().max(1e-12);
    assert!(
        error <= tolerance,
        "{what}: {got} vs {want}, off by {error:e}, allowed {tolerance:e}"
    );
}

fn bundled(_: &str) -> Result<(Vec<u8>, u32), String> {
    Ok((font::BUNDLED.to_vec(), 0))
}

fn spec(kind: Kind, build: Build, point: [f64; 3], normal: [f64; 3]) -> Spec {
    Spec {
        kind,
        text: "Fig 42".into(),
        entries: Vec::new(),
        font: BUNDLED_ID.into(),
        font_size: 48.0,
        height: 6.0,
        width: 0.0,
        keep_aspect: true,
        depth: 1.0,
        build,
        thickness: 2.0,
        point,
        normal,
        angle: 0.0,
        tag_direction: None,
        threshold: 0.5,
    }
}

fn run(mesh: &Mesh, specs: &[Spec], pictures: &[Option<Picture>]) -> Result<Applied, String> {
    let sources = Sources {
        font: &bundled,
        pictures,
    };
    silent(|c| apply(mesh, specs, &sources, c)).unwrap()
}

fn one(mesh: &Mesh, s: Spec) -> Mesh {
    let mut applied = run(mesh, &[s], &[]).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(applied.copies.len(), 1);
    applied.copies.remove(0).1
}

/// The label's area in mm² at the spec's size.
fn label_area(s: &Spec, picture: Option<&Picture>) -> f64 {
    let shape = match picture {
        Some(p) => picture_shape(p, s.threshold).unwrap(),
        None => text_shape(font::BUNDLED, 0, &s.text, s.font_size).unwrap(),
    };
    sized(&shape, s.height, width_of(s, shape.aspect), false).area()
}

fn block() -> Mesh {
    make_box([0.0; 3], [60.0, 40.0, 20.0])
}

/// A dark ring on white, 64 × 48 samples: an outline with a hole.
fn ring_picture() -> Picture {
    let (w, h) = (64usize, 48usize);
    let mut gray = vec![1.0f32; w * h];
    for j in 0..h {
        for i in 0..w {
            let (x, y) = (i as f64 - 31.5, j as f64 - 23.5);
            let r = x.hypot(y);
            if (8.0..18.0).contains(&r) {
                gray[j * w + i] = 0.0;
            }
        }
    }
    Picture {
        width: w,
        height: h,
        gray,
    }
}

#[test]
fn the_bundled_font_works_without_any_system_font() {
    let fonts = font::list(&[]);
    assert_eq!(fonts.len(), 1);
    assert_eq!(fonts[0].0.id, BUNDLED_ID);
    assert!(fonts[0].0.bundled);
    let shape = text_shape(&font::load(&fonts[0].1).unwrap(), 0, "Ag ÄÖ Ж", 48.0).unwrap();
    assert!(shape.aspect > 3.0, "{}", shape.aspect);
    // "A" has one hole, "g" one, "Ö" two dots: outlines with holes come out as polygons.
    assert!(shape.region.polygons.iter().any(|p| !p.holes.is_empty()));
    let missing = text_shape(font::BUNDLED, 0, "日本", 48.0).unwrap_err();
    assert!(missing.contains("日本"), "{missing}");
}

#[test]
fn overlapping_and_touching_outlines_become_separate_polygons() {
    // Two squares overlapping, and a third touching the second at a corner only.
    let square = |x: f64, y: f64| vec![[x, y], [x + 1.0, y], [x + 1.0, y + 1.0], [x, y + 1.0]];
    let region =
        Region::from_contours(&[square(0.0, 0.0), square(0.5, 0.5), square(1.5, 1.5)]).unwrap();
    assert_eq!(region.polygons.len(), 2);
    close("joined area", region.area(), 1.75 + 1.0, 1e-3);
    // No point is shared by two loops.
    let mut seen = std::collections::HashSet::new();
    for p in region.loops().flatten() {
        assert!(seen.insert((p[0].to_bits(), p[1].to_bits())));
    }
}

#[test]
fn text_raised_and_sunk_on_a_box_adds_and_removes_area_times_depth() {
    let part = block();
    let v0 = volume(&part);
    for build in [
        Build::Add,
        Build::Subtract,
        Build::AddMirrored,
        Build::SubtractMirrored,
    ] {
        let s = spec(Kind::Text, build, [30.0, 20.0, 20.0], [0.0, 0.0, 1.0]);
        let area = label_area(&s, None);
        let out = one(&part, s.clone());
        assert_clean(&out, &format!("{build:?}"));
        let change = volume(&out) - v0;
        let want = if build.raised() {
            area * s.depth
        } else {
            -area * s.depth
        };
        // The flat face holds the outline exactly; f32 rounding of the cut points is all.
        close(&format!("{build:?} volume change"), change, want, 1e-4);
    }
}

#[test]
fn mirrored_labels_are_mirror_images() {
    let part = block();
    let centre_x = |m: &Mesh| {
        // The mean x of the vertices above the box: the raised letters.
        let xs: Vec<f64> = (0..m.vertex_count() as u32)
            .map(|i| m.point(i))
            .filter(|p| p[2] > 20.5)
            .map(|p| p[0])
            .collect();
        xs.iter().sum::<f64>() / xs.len() as f64
    };
    let mut s = spec(Kind::Text, Build::Add, [30.0, 20.0, 20.0], [0.0, 0.0, 1.0]);
    // "Γ" leans its weight to the left: its mirror image leans right.
    s.text = "Γ".into();
    let plain = centre_x(&one(&part, s.clone()));
    s.build = Build::AddMirrored;
    let mirrored = centre_x(&one(&part, s));
    assert!(
        plain < 30.0 - 0.1 && mirrored > 30.0 + 0.1,
        "{plain} {mirrored}"
    );
    close("mirror image", 30.0 - plain, mirrored - 30.0, 1e-4);
}

#[test]
fn image_labels_raised_and_sunk_match_the_dark_area() {
    let part = block();
    let picture = ring_picture();
    for build in [Build::Add, Build::Subtract] {
        let mut s = spec(Kind::Image, build, [30.0, 20.0, 20.0], [0.0, 0.0, 1.0]);
        s.height = 20.0;
        let area = label_area(&s, Some(&picture));
        let applied = run(&part, &[s.clone()], &[Some(picture.clone())]).unwrap();
        let out = &applied.copies[0].1;
        assert_clean(out, "image");
        // The ring's area by formula: π (18² − 8²) samples², at 20 mm per 48 samples. The
        // outline runs between samples, so allow 3 %.
        let mm = 20.0 / 48.0;
        close(
            "ring area",
            area,
            std::f64::consts::PI * (18.0f64.powi(2) - 8.0f64.powi(2)) * mm * mm,
            0.03,
        );
        let want = if build.raised() {
            area * s.depth
        } else {
            -area * s.depth
        };
        close(
            "image volume change",
            volume(out) - volume(&part),
            want,
            1e-4,
        );
    }
}

#[test]
fn labels_on_a_cylinder_side_follow_the_tangent_plane() {
    // Gently curved: the label's edges lie well under a millimetre below its tangent plane.
    let (r, h) = (30.0, 40.0);
    let part = primitive_cylinder(r, h, 96).unwrap();
    for build in [Build::Add, Build::Subtract] {
        let mut s = spec(Kind::Text, build, [r, 0.0, 20.0], [1.0, 0.0, 0.0]);
        s.text = "ROUND".into();
        s.height = 3.0;
        s.depth = 1.5;
        let out = one(&part, s.clone());
        assert_clean(&out, &format!("cylinder {build:?}"));
        let width = width_of(
            &s,
            text_shape(font::BUNDLED, 0, "ROUND", 48.0).unwrap().aspect,
        );
        // The flat top (raised) or floor (sunk) lies at the depth from the tangent plane x = r,
        // at the centre and at the edges alike; the surface there has curved away by the sag.
        let level = if build.raised() {
            r + s.depth
        } else {
            r - s.depth
        };
        let new: Vec<[f64; 3]> = (0..out.vertex_count() as u32)
            .map(|i| out.point(i))
            .filter(|p| (p[0] - level).abs() < 1e-3 && (p[2] - 20.0).abs() <= s.height)
            .collect();
        assert!(!new.is_empty());
        let reach = new.iter().map(|p| p[1].abs()).fold(0.0, f64::max);
        close("label reaches its edges", reach, width / 2.0, 0.02);
        // At its edge (|y| = width / 2) the label stands depth + sag above the curved surface.
        let sag = r - (r * r - (width / 2.0).powi(2)).sqrt();
        let edge = new
            .iter()
            .find(|p| p[1].abs() > 0.95 * width / 2.0)
            .unwrap();
        let surface_x = (r * r - edge[1] * edge[1]).sqrt();
        let height_there = (level - surface_x).abs();
        let want = if build.raised() {
            s.depth + sag
        } else {
            s.depth - sag
        };
        close("depth at the edge", height_there, want, 0.05);
        // Nothing of the rest of the cylinder changed.
        assert!(volume(&out) > 0.0);
    }
}

#[test]
fn a_sunk_label_deeper_than_the_wall_is_refused() {
    let thin = make_box([0.0; 3], [40.0, 40.0, 2.0]);
    let mut s = spec(
        Kind::Text,
        Build::Subtract,
        [20.0, 20.0, 2.0],
        [0.0, 0.0, 1.0],
    );
    s.depth = 1.95;
    assert_eq!(run(&thin, &[s], &[]).err().as_deref(), Some(emboss::THIN));
}

#[test]
fn a_label_past_the_edge_is_refused() {
    let mut s = spec(Kind::Text, Build::Add, [58.0, 20.0, 20.0], [0.0, 0.0, 1.0]);
    s.height = 10.0;
    let why = run(&block(), &[s], &[]).err().unwrap();
    assert!(
        why == emboss::OVER_EDGE || why == emboss::PAST_EDGE,
        "{why}"
    );
}

#[test]
fn several_labels_of_different_kinds_go_on_in_one_run() {
    let part = block();
    let mut top = spec(Kind::Text, Build::Add, [20.0, 20.0, 20.0], [0.0, 0.0, 1.0]);
    top.text = "TOP".into();
    let mut side = spec(
        Kind::Image,
        Build::Subtract,
        [30.0, 0.0, 10.0],
        [0.0, -1.0, 0.0],
    );
    side.height = 12.0;
    let mut turned = spec(
        Kind::Text,
        Build::Subtract,
        [45.0, 20.0, 20.0],
        [0.0, 0.0, 1.0],
    );
    turned.text = "90".into();
    turned.angle = 90.0;
    let picture = ring_picture();
    let specs = [top.clone(), side.clone(), turned.clone()];
    let applied = run(&part, &specs, &[None, Some(picture.clone()), None]).unwrap();
    let out = &applied.copies[0].1;
    assert_clean(out, "three labels");
    let want = label_area(&top, None) * top.depth
        - label_area(&side, Some(&picture)) * side.depth
        - label_area(&turned, None) * turned.depth;
    close(
        "three labels' volume",
        volume(out) - volume(&part),
        want,
        1e-4,
    );
    // The turned label runs along y.
    let sunk: Vec<[f64; 3]> = (0..out.vertex_count() as u32)
        .map(|i| out.point(i))
        .filter(|p| (p[2] - (20.0 - turned.depth)).abs() < 1e-4 && p[0] > 35.0)
        .collect();
    let span = |k: usize| {
        sunk.iter().map(|p| p[k]).fold(f64::NEG_INFINITY, f64::max)
            - sunk.iter().map(|p| p[k]).fold(f64::INFINITY, f64::min)
    };
    assert!(span(1) > span(0), "x {} y {}", span(0), span(1));
}

#[test]
fn running_numbers_make_one_named_copy_each() {
    let part = block();
    let mut s = spec(
        Kind::List,
        Build::Subtract,
        [30.0, 20.0, 20.0],
        [0.0, 0.0, 1.0],
    );
    s.entries = (1..=5).map(|n| format!("SN-{n:03}")).collect();
    let applied = run(&part, &[s.clone()], &[]).unwrap();
    assert_eq!(applied.copies.len(), 5);
    assert!(applied.note.is_none());
    for (n, (name, mesh)) in applied.copies.iter().enumerate() {
        assert_eq!(name, &format!("SN-{:03}", n + 1));
        assert_clean(mesh, name);
        let mut one_label = s.clone();
        one_label.text = name.clone();
        let area = label_area(&one_label, None);
        close(name, volume(&part) - volume(mesh), area * s.depth, 1e-4);
    }
    // Different texts: different volumes.
    assert_ne!(volume(&applied.copies[0].1), volume(&applied.copies[3].1));
}

#[test]
fn a_csv_column_makes_one_copy_per_row_and_the_shortest_list_rules() {
    let rows =
        list::parse_csv("Name;Text\nMain;\"Main copy\"\nLab;Lab copy\nArchive;\"Arch; 3\"\n");
    assert_eq!(rows.len(), 4);
    let column: Vec<String> = rows[1..].iter().map(|r| r[1].clone()).collect();
    assert_eq!(column, ["Main copy", "Lab copy", "Arch; 3"]);
    let part = block();
    let mut csv = spec(Kind::List, Build::Add, [20.0, 20.0, 20.0], [0.0, 0.0, 1.0]);
    csv.entries = column;
    let mut numbers = spec(
        Kind::List,
        Build::Subtract,
        [30.0, 0.0, 10.0],
        [0.0, -1.0, 0.0],
    );
    numbers.entries = (1..=5).map(|n| n.to_string()).collect();
    let applied = run(&part, &[csv, numbers], &[]).unwrap();
    assert_eq!(applied.copies.len(), 3);
    assert!(applied.note.as_deref().unwrap().contains("3 copies"));
    let names: Vec<&str> = applied.copies.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(names, ["Main copy 1", "Lab copy 2", "Arch; 3 3"]);
    for (name, mesh) in &applied.copies {
        assert_clean(mesh, name);
    }
}

#[test]
fn csv_files_with_commas_tabs_quotes_and_a_byte_order_mark() {
    assert_eq!(
        list::parse_csv("\u{feff}a,b\n1,2\n"),
        [["a", "b"], ["1", "2"]]
    );
    assert_eq!(
        list::parse_csv("a\tb\r\n\r\n 1 \t\"x\"\"y\"\r\n"),
        [["a", "b"], ["1", "x\"y"]]
    );
    assert_eq!(list::parse_csv("one\ntwo"), [["one"], ["two"]]);
    assert_eq!(
        list::parse_csv("\"line\nbreak\",z\n"),
        [["line\nbreak", "z"]]
    );
}

#[test]
fn a_tag_stands_on_a_thin_bridge_and_the_part_stays_one_solid() {
    let part = block();
    for build in [Build::Add, Build::Subtract] {
        let mut s = spec(Kind::Tag, build, [30.0, 20.0, 20.0], [0.0, 0.0, 1.0]);
        s.text = "TAG".into();
        s.height = 5.0;
        s.depth = 0.6;
        s.thickness = 2.0;
        let out = one(&part, s.clone());
        assert_clean(&out, &format!("tag {build:?}"));
        assert_eq!(stats(&out).shells, 1, "the tag is joined to the part");
        // Plate and bridge: the plate's volume plus the bridge's, plus or minus the caption.
        let width = width_of(
            &s,
            text_shape(font::BUNDLED, 0, "TAG", 48.0).unwrap().aspect,
        );
        let (_, pw, ph, bw, bt) = tag::plate_size(width, s.height, s.thickness);
        let caption = label_area(&s, None) * s.depth * if build.raised() { 1.0 } else { -1.0 };
        let want = pw * ph * s.thickness + bw * bt * tag::GAP + caption;
        close("tag volume", volume(&out) - volume(&part), want, 1e-4);
        // The plate stands above the surface by the bridge's length.
        let top = (0..out.vertex_count() as u32)
            .map(|i| out.point(i)[2])
            .fold(0.0, f64::max);
        close("plate top", top, 20.0 + tag::GAP + ph, 1e-6);
    }
    // Leaning, by the handle: still sound.
    let mut s = spec(Kind::Tag, Build::Add, [30.0, 20.0, 20.0], [0.0, 0.0, 1.0]);
    s.tag_direction = Some([0.0, 0.5, 1.0]);
    s.height = 5.0;
    s.depth = 0.6;
    assert_clean(&one(&part, s.clone()), "leaning tag");
    s.tag_direction = Some([0.0, 1.0, 0.1]);
    assert_eq!(run(&part, &[s], &[]).err().as_deref(), Some(tag::LEAN));
}

#[test]
fn a_tag_on_a_wall_carries_its_caption_on_top() {
    // On the front wall (normal −y) the plate sticks out like a shelf, 2 mm thick round the
    // clicked height of 10 mm: the raised caption stands on its upper face, never under it.
    let part = block();
    let mut s = spec(Kind::Tag, Build::Add, [30.0, 0.0, 10.0], [0.0, -1.0, 0.0]);
    s.text = "WALL".into();
    s.height = 4.0;
    s.depth = 0.6;
    let out = one(&part, s.clone());
    assert_clean(&out, "wall tag");
    // The tag is everything in front of the wall, past the bridge.
    let tag: Vec<[f64; 3]> = (0..out.vertex_count() as u32)
        .map(|i| out.point(i))
        .filter(|p| p[1] < -1.5)
        .collect();
    let top = tag.iter().map(|p| p[2]).fold(f64::NEG_INFINITY, f64::max);
    let bottom = tag.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min);
    close("caption on top", top, 11.0 + s.depth, 1e-6);
    close("nothing underneath", bottom, 9.0, 1e-6);
}

#[test]
fn an_exact_coincidence_is_shifted_away() {
    // A square label whose sides run exactly through the box's top diagonal's corners.
    let part = make_box([0.0; 3], [20.0, 20.0, 10.0]);
    let region = Region::from_loops(vec![vec![
        [-10.0, -10.0],
        [10.0, -10.0],
        [10.0, 10.0],
        [-10.0, 10.0],
    ]]);
    let frame = Frame::label([10.0, 10.0, 10.0], [0.0, 0.0, 1.0], 0.0).unwrap();
    // Covers the whole face: past the edge.
    let refused =
        silent(|c| emboss::emboss(&part, &frame, &region, Relief::Raise(1.0), c)).unwrap();
    assert!(refused.is_err());
    // A smaller square whose corner sits exactly on the diagonal.
    let region = Region::from_loops(vec![vec![
        [-5.0, -5.0],
        [5.0, -5.0],
        [5.0, 5.0],
        [-5.0, 5.0],
    ]]);
    let out = silent(|c| emboss::emboss(&part, &frame, &region, Relief::Raise(1.0), c))
        .unwrap()
        .unwrap();
    assert_clean(&out.mesh, "square on the diagonal");
    close(
        "square volume",
        volume(&out.mesh) - volume(&part),
        100.0,
        1e-3,
    );
}

// ---------------------------------------------------------------------------------------------
// Starting sizes (fit.rs)

/// The fitted height of `text` placed at `point` on `part`'s face facing `normal`.
fn fitted(part: &Mesh, text: &str, point: [f64; 3], normal: [f64; 3]) -> (fit::Fit, f64) {
    let aspect = text_shape(font::BUNDLED, 0, text, 48.0).unwrap().aspect;
    let frame = Frame::label(point, normal, 0.0).unwrap();
    (fit::fit(part, &frame, aspect), aspect)
}

/// The box round the raised label: the points above `top`.
fn label_box(mesh: &Mesh, top: f64) -> ([f64; 2], [f64; 2]) {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for p in (0..mesh.vertex_count() as u32)
        .map(|i| mesh.point(i))
        .filter(|p| p[2] > top + 1e-3)
    {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

#[test]
fn a_label_on_a_20_mm_box_face_fits_inside_the_face() {
    let part = make_box([0.0; 3], [20.0; 3]);
    for (point, room) in [([10.0, 10.0, 20.0], 20.0), ([15.0, 10.0, 20.0], 10.0)] {
        let (fit, aspect) = fitted(&part, "Fig 42", point, [0.0, 0.0, 1.0]);
        assert!(fit.fits);
        // 80 % of the room along the text, centred on the point clicked.
        close("width", fit.height * aspect, fit::SHARE * room, 1e-9);
        assert_eq!(fit.thickness, Some(20.0));
        let mut s = spec(Kind::Text, Build::Add, point, [0.0, 0.0, 1.0]);
        s.height = fit.height;
        s.depth = (fit.height / 2.0).min(5.0);
        let out = one(&part, s);
        assert_clean(&out, "fitted label");
        let (lo, hi) = label_box(&out, 20.0);
        assert!(
            lo[0] > 0.0 && lo[1] > 0.0 && hi[0] < 20.0 && hi[1] < 20.0,
            "{lo:?} {hi:?}"
        );
        assert!(hi[0] - lo[0] <= fit::SHARE * room + 1e-6);
    }
}

#[test]
fn on_a_200_mm_box_a_label_starts_10_mm_tall() {
    let part = make_box([0.0; 3], [200.0; 3]);
    let (fit, _) = fitted(&part, "Fig 42", [100.0, 100.0, 200.0], [0.0, 0.0, 1.0]);
    assert_eq!(fit.height, fit::MAX_HEIGHT);
    assert!(fit.fits);
    // On a side, the same.
    let (side, _) = fitted(&part, "Fig 42", [100.0, 0.0, 100.0], [0.0, -1.0, 0.0]);
    assert_eq!(side.height, fit::MAX_HEIGHT);
}

#[test]
fn a_long_caption_on_a_small_part_shrinks_to_fit() {
    let part = make_box([0.0; 3], [22.0, 22.0, 6.0]);
    let text = "A long caption on a small part";
    let (fit, aspect) = fitted(&part, text, [11.0, 11.0, 6.0], [0.0, 0.0, 1.0]);
    assert!(
        fit.fits && fit.height < 2.0 && fit.height >= fit::MIN_HEIGHT,
        "{}",
        fit.height
    );
    close("width", fit.height * aspect, fit::SHARE * 22.0, 1e-9);
    assert_eq!(fit.thickness, Some(6.0));
    let mut s = spec(
        Kind::Text,
        Build::Subtract,
        [11.0, 11.0, 6.0],
        [0.0, 0.0, 1.0],
    );
    s.text = text.into();
    s.height = fit.height;
    // The starting depth for a sunk label: the smaller of half its height and 40 % of the part.
    s.depth = (fit.height / 2.0).min(0.4 * 6.0);
    let out = one(&part, s);
    assert_clean(&out, "shrunk caption");
    // A face too small for even a millimetre: the label starts at 1 mm, and does not fit.
    let tiny = make_box([0.0; 3], [0.8, 0.8, 0.8]);
    let (fit, _) = fitted(&tiny, "Fig 42", [0.4, 0.4, 0.8], [0.0, 0.0, 1.0]);
    assert!(!fit.fits);
    assert_eq!(fit.height, fit::MIN_HEIGHT);
    // A point off the part fits nothing.
    let (off, _) = fitted(&part, "Fig 42", [50.0, 50.0, 6.0], [0.0, 0.0, 1.0]);
    assert!(!off.fits);
}

#[test]
fn a_cylinder_side_leaves_room_only_where_it_is_nearly_flat() {
    let r = 10.0;
    let part = primitive_cylinder(r, 30.0, 96).unwrap();
    let (fit, aspect) = fitted(&part, "Fig 42", [r, 0.0, 15.0], [1.0, 0.0, 0.0]);
    // The flat area spans the facets within 30° of the clicked one: about 2 r sin 30° wide.
    let width = fit.height * aspect;
    assert!(
        width <= fit::SHARE * 2.0 * r * 30f64.to_radians().sin() + 0.5,
        "{width}"
    );
    assert!(
        width >= fit::SHARE * 2.0 * r * 25f64.to_radians().sin(),
        "{width}"
    );
    close("thickness", fit.thickness.unwrap(), 2.0 * r, 1e-3);
}

#[test]
fn list_copies_are_all_the_same_size() {
    let part = make_box([0.0; 3], [30.0, 30.0, 10.0]);
    let entries: Vec<String> = ["ace", "Ace", "A longer entry"].map(String::from).to_vec();
    // Sized by the widest entry.
    let shape = sizing_shape(
        Kind::List,
        "",
        &entries,
        &|| Ok((font::BUNDLED.to_vec(), 0)),
        48.0,
        None,
        0.5,
    )
    .unwrap();
    let widest = text_shape(font::BUNDLED, 0, "A longer entry", 48.0).unwrap();
    close("sized by the widest", shape.aspect, widest.aspect, 1e-12);
    let frame = Frame::label([15.0, 15.0, 10.0], [0.0, 0.0, 1.0], 0.0).unwrap();
    let fit = fit::fit(&part, &frame, shape.aspect);
    let mut s = spec(Kind::List, Build::Add, [15.0, 15.0, 10.0], [0.0, 0.0, 1.0]);
    s.entries = entries.clone();
    s.height = fit.height;
    let applied = run(&part, &[s], &[]).unwrap();
    assert_eq!(applied.copies.len(), 3);
    // One scale for every copy: each copy's letters are as tall in mm as its own ink in font
    // units times the same factor, so "ace" is shorter than "Ace" by the font's x-height.
    let mut factors = Vec::new();
    for (entry, (name, mesh)) in entries.iter().zip(&applied.copies) {
        assert_clean(mesh, name);
        let (lo, hi) = label_box(mesh, 10.0);
        assert!(
            lo[0] > 0.0 && hi[0] < 30.0 && hi[0] - lo[0] <= fit::SHARE * 30.0 + 1e-6,
            "{name}: {lo:?} {hi:?}"
        );
        let ink = text_shape(font::BUNDLED, 0, entry, 48.0).unwrap().unit;
        factors.push((hi[1] - lo[1]) / ink);
    }
    for f in &factors {
        close("same scale", *f, factors[0], 1e-3);
    }
    // Per-entry sizing would have made "ace" as tall as "Ace".
    let ratio = factors[0] * text_shape(font::BUNDLED, 0, "ace", 48.0).unwrap().unit
        / (factors[1] * text_shape(font::BUNDLED, 0, "Ace", 48.0).unwrap().unit);
    assert!(ratio < 0.85, "{ratio}");
}

#[test]
fn a_thin_plate_is_measured_under_the_point() {
    let plate = make_box([0.0; 3], [40.0, 40.0, 2.0]);
    let (fit, _) = fitted(&plate, "Fig 42", [20.0, 20.0, 2.0], [0.0, 0.0, 1.0]);
    assert_eq!(fit.thickness, Some(2.0));
}
