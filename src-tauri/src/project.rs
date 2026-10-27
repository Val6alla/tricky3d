//! tricky3D projects: a 3MF package (unit millimetre) with one object per part in part-local
//! coordinates and one build item per part carrying the part's matrix as its `transform`. The
//! platform size and each part's display fields travel as metadata in the tricky3D namespace,
//! so any 3MF program can still open a project, and a 3MF from elsewhere opens as a project.

use std::io::{self, Seek, Write};
use std::path::Path;

use crate::formats::threemf::{self, ModelOut, ObjectOut, Package};
use crate::formats::{display_name, has_extension, read_all, write_atomic, write_error};
use crate::mesh::{Matrix, Mesh};
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
    pub locked: bool,
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
    let objects = parts
        .iter()
        .map(|p| {
            let mut metadata = Vec::new();
            if let Some(color) = p.color {
                metadata.push(("tricky3d:color", color.to_string()));
            }
            metadata.push(("tricky3d:visible", p.visible.to_string()));
            metadata.push(("tricky3d:transparent", p.transparent.to_string()));
            metadata.push(("tricky3d:locked", p.locked.to_string()));
            ObjectOut {
                name: p.name,
                mesh: p.mesh,
                transform: Some(p.matrix),
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
            Part {
                name: crate::formats::clean_name(item.object_name.as_deref())
                    .unwrap_or_else(|| format!("Part {}", n + 1)),
                matrix: item.matrix,
                color: field("color").filter(|c| !c.is_empty() && c.len() <= 64),
                visible: flag("visible"),
                transparent: flag("transparent"),
                locked: flag("locked"),
                mesh: item.mesh,
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
