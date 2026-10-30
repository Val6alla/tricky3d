//! tricky3D projects: a 3MF package (unit millimetre) with one object per part and one build item
//! per part. Each object holds the part where it sits on the platform (its matrix baked into the
//! vertices) and its build item has no transform, so every 3MF program places every part where
//! tricky3D showed it. The part's own matrix, the platform size, each part's display fields, its
//! packing settings and the file it was imported from travel as metadata in the tricky3D
//! namespace; opening a project takes the matrix back out of the vertices.
//!
//! Projects saved before kept the part-local mesh and put the matrix in the build item's
//! `transform`. They still open the same, and so does a 3MF from elsewhere.

use std::io::{self, Seek, Write};
use std::path::Path;

use crate::arrange::pack::Rotation;
use crate::formats::threemf::{self, ModelOut, ObjectOut, Package};
use crate::formats::{display_name, has_extension, read_all, write_atomic, write_error};
use crate::measure::invert;
use crate::mesh::{IDENTITY, Matrix, Mesh, bake, multiply};
use crate::progress::Progress;

pub const NAMESPACE: &str = "urn:tricky3d:project:2026";
const PREFIX: &str = "tricky3d";
/// The namespace of projects saved before the app was renamed from Meshbench to tricky3D. Read,
/// never written.
const OLD_NAMESPACE: &str = "urn:meshbench:project:2026";

/// A part going into a project file.
pub struct PartOut<'a> {
    pub name: &'a str,
    pub matrix: Matrix,
    pub color: Option<&'a str>,
    pub visible: bool,
    pub transparent: bool,
    /// Also its packing position lock.
    pub locked: bool,
    /// Its own packing priority (0 to 10) and rotation, or None for the defaults.
    pub priority: Option<u8>,
    pub rotation: Option<Rotation>,
    /// Measure mode's measurements on the part, as the window's JSON (src/measure), or None.
    pub measurements: Option<&'a str>,
    /// The file the part (or the part it was made from) was imported from, or None.
    pub source: Option<&'a str>,
    /// Part-local, as the window holds it; written with `matrix` baked in.
    pub mesh: &'a Mesh,
}

/// A part read from a project; display fields are None when the file does not say.
#[derive(Debug)]
pub struct Part {
    pub name: String,
    pub matrix: Matrix,
    pub color: Option<String>,
    pub visible: Option<bool>,
    pub transparent: Option<bool>,
    pub locked: Option<bool>,
    pub priority: Option<u8>,
    pub rotation: Option<Rotation>,
    /// The measurements' JSON as saved; the window checks it.
    pub measurements: Option<String>,
    /// The file the part was imported from, as saved.
    pub source: Option<String>,
    /// Part-local: `matrix` places it.
    pub mesh: Mesh,
}

#[derive(Debug)]
pub struct Project {
    /// From tricky3d:platform; None for a 3MF written by another program.
    pub platform: Option<[f64; 3]>,
    pub parts: Vec<Part>,
    pub warnings: Vec<String>,
}

pub fn write<W: Write + Seek>(w: W, platform: [f64; 3], parts: &[PartOut]) -> io::Result<W> {
    let baked: Vec<Mesh> = parts.iter().map(|p| bake(p.mesh, &p.matrix)).collect();
    let objects = parts
        .iter()
        .zip(&baked)
        .map(|(p, mesh)| {
            let mut metadata = vec![("tricky3d:matrix", format_matrix(&p.matrix))];
            if let Some(color) = p.color {
                metadata.push(("tricky3d:color", color.to_string()));
            }
            metadata.push(("tricky3d:visible", p.visible.to_string()));
            metadata.push(("tricky3d:transparent", p.transparent.to_string()));
            metadata.push(("tricky3d:locked", p.locked.to_string()));
            if let Some(priority) = p.priority {
                metadata.push(("tricky3d:packing-priority", priority.to_string()));
            }
            if let Some(rotation) = p.rotation {
                metadata.push(("tricky3d:packing-rotation", rotation.word().to_string()));
            }
            if let Some(json) = p.measurements {
                metadata.push(("tricky3d:measurements", json.to_string()));
            }
            if let Some(source) = p.source {
                metadata.push(("tricky3d:source", source.to_string()));
            }
            ObjectOut {
                name: p.name,
                mesh,
                transform: None,
                metadata,
            }
        })
        .collect();
    let platform_text = format!("{} {} {}", platform[0], platform[1], platform[2]);
    let model = ModelOut {
        namespaces: &[(PREFIX, NAMESPACE)],
        metadata: vec![
            ("Application", threemf::application()),
            ("tricky3d:platform", platform_text),
        ],
        objects,
    };
    threemf::write(w, &model)
}

/// Saves a project to `path` with a message naming the file on failure.
pub fn save_file(path: &Path, platform: [f64; 3], parts: &[PartOut]) -> Result<(), String> {
    let file = display_name(path);
    if !has_extension(path, "3mf") {
        return Err(format!(
            "Could not save \"{file}\": the file name must end in .3mf."
        ));
    }
    write_atomic(path, |w| write(w, platform, parts))
        .map_err(|e| format!("Could not save \"{file}\": {}.", write_error(&e)))
}

/// Longest measurements text a part may carry (bytes); anything longer is not ours.
const MAX_MEASUREMENTS: usize = 4 << 20;

/// Longest source path a part may carry (bytes).
const MAX_SOURCE: usize = 4096;

/// Highest packing priority (the lowest is 0).
pub const MAX_PRIORITY: u8 = 10;

/// The 16 numbers of a column-major matrix, in shortest round-trip form.
fn format_matrix(m: &Matrix) -> String {
    m.iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A matrix written by `format_matrix`: 16 finite numbers, affine (bottom row 0 0 0 1), and not
/// squashed flat. None otherwise.
fn parse_matrix(text: &str) -> Option<Matrix> {
    let values: Vec<f64> = text
        .split_whitespace()
        .map(|v| v.parse::<f64>().ok().filter(|v| v.is_finite()))
        .collect::<Option<_>>()?;
    let m = Matrix::try_from(values.as_slice()).ok()?;
    let affine = m[3] == 0.0 && m[7] == 0.0 && m[11] == 0.0 && m[15] == 1.0;
    (affine && invert(&m).is_some()).then_some(m)
}

/// The part's own mesh and matrix from what a project holds: `world` placed by the build item's
/// `transform`, and the matrix saved in tricky3d:matrix (None for a project saved before that,
/// or a 3MF from elsewhere, whose transform then is the part's matrix).
fn part_frame(world: Mesh, transform: Matrix, saved: Option<Matrix>) -> (Mesh, Matrix) {
    match saved {
        Some(matrix) => {
            let back = invert(&matrix).expect("parse_matrix only accepts invertible matrices");
            let local = multiply(&back, &transform);
            let mesh = if local == IDENTITY {
                world
            } else {
                bake(&world, &local)
            };
            (mesh, matrix)
        }
        None => (world, transform),
    }
}

/// The names a project field may have in the metadata: ours, then the pre-rename one.
fn keys(local: &str) -> [String; 2] {
    [NAMESPACE, OLD_NAMESPACE].map(|ns| format!("{{{ns}}}{local}"))
}

pub fn read(bytes: &[u8], progress: &mut Progress) -> Result<Project, String> {
    let package: Package = threemf::read(bytes, progress)?;
    let mut warnings = package.warnings.clone();
    let platform = keys("platform")
        .iter()
        .find_map(|k| package.metadata_value(k));
    let platform = platform.and_then(|text| {
        let values: Option<Vec<f64>> = text
            .split_whitespace()
            .map(|v| v.parse().ok().filter(|v: &f64| v.is_finite() && *v > 0.0))
            .collect();
        match values.as_deref() {
            Some(&[x, y, z]) => Some([x, y, z]),
            _ => {
                warnings.push(
                    "The platform size in the project is not valid, so it was not changed.".into(),
                );
                None
            }
        }
    });
    let parts = package
        .items
        .into_iter()
        .enumerate()
        .map(|(n, item)| {
            let field = |local: &str| {
                let keys = keys(local);
                item.metadata
                    .iter()
                    .find(|(k, _)| keys.contains(k))
                    .map(|(_, v)| v.trim().to_string())
            };
            let flag = |local: &str| {
                field(local).and_then(|v| match v.as_str() {
                    "true" | "1" => Some(true),
                    "false" | "0" => Some(false),
                    _ => None,
                })
            };
            let (mesh, matrix) = part_frame(
                item.mesh,
                item.matrix,
                field("matrix").as_deref().and_then(parse_matrix),
            );
            Part {
                name: crate::formats::clean_name(item.object_name.as_deref())
                    .unwrap_or_else(|| format!("Part {}", n + 1)),
                matrix,
                color: field("color").filter(|c| !c.is_empty() && c.len() <= 64),
                visible: flag("visible"),
                transparent: flag("transparent"),
                locked: flag("locked"),
                priority: field("packing-priority")
                    .and_then(|v| v.parse::<u8>().ok())
                    .filter(|p| *p <= MAX_PRIORITY),
                rotation: field("packing-rotation")
                    .as_deref()
                    .and_then(Rotation::from_word),
                measurements: field("measurements")
                    .filter(|m| !m.is_empty() && m.len() <= MAX_MEASUREMENTS),
                source: field("source")
                    .filter(|s| !s.is_empty() && s.len() <= MAX_SOURCE && !s.contains('\0')),
                mesh,
            }
        })
        .collect::<Vec<_>>();
    // A tricky3D project may hold no parts (just the platform); anything else needs triangles.
    if parts.is_empty() && platform.is_none() {
        return Err("it has no build items, so there is nothing to open".into());
    }
    if !parts.is_empty() && parts.iter().all(|p| p.mesh.indices.is_empty()) {
        return Err("it has no triangles".into());
    }
    Ok(Project {
        platform,
        parts,
        warnings,
    })
}

/// Opens a project with a message naming the file on failure.
pub fn open_file(path: &Path, progress: &mut Progress) -> Result<Project, String> {
    let fail = |reason: String| format!("Could not open \"{}\": {reason}.", display_name(path));
    let bytes = read_all(path, progress).map_err(fail)?;
    read(&bytes, progress).map_err(fail)
}
