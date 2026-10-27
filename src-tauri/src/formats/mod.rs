//! Mesh file formats: detection, reading into welded meshes, and writing. Readers return a reason
//! (a lower-case clause such as "the file ends early") and `import_file` / `export_file` turn it
//! into the full sentence the user sees, naming the file.

pub mod amf;
pub mod obj;
pub mod off;
pub mod ply;
pub mod stl;
pub mod threemf;
pub(crate) mod xml;

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::mesh::{Matrix, Mesh};
use crate::progress::{Progress, Stage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportFormat {
    Stl,
    Obj,
    Ply,
    ThreeMf,
    Amf,
    Off,
}

impl ImportFormat {
    /// Every format tricky3D reads, in menu order.
    pub const ALL: [ImportFormat; 6] = [
        ImportFormat::Stl,
        ImportFormat::Obj,
        ImportFormat::Ply,
        ImportFormat::ThreeMf,
        ImportFormat::Amf,
        ImportFormat::Off,
    ];

    /// The format for `path`'s extension, in any letter case.
    pub fn from_path(path: &Path) -> Option<ImportFormat> {
        let ext = path.extension()?.to_str()?;
        ImportFormat::ALL
            .into_iter()
            .find(|f| f.id().eq_ignore_ascii_case(ext))
    }

    /// The `format` field of the import result (src/io/ipc.ts `ImportFormat`), which is also the
    /// format's file extension.
    pub fn id(self) -> &'static str {
        match self {
            ImportFormat::Stl => "stl",
            ImportFormat::Obj => "obj",
            ImportFormat::Ply => "ply",
            ImportFormat::ThreeMf => "3mf",
            ImportFormat::Amf => "amf",
            ImportFormat::Off => "off",
        }
    }
}

/// One object a reader found, already welded, before it gets its part name.
#[derive(Debug)]
pub struct Object {
    /// The name the file gives the object, if any.
    pub name: Option<String>,
    /// Placement from the file (3MF build items); None means identity.
    pub matrix: Option<Matrix>,
    pub mesh: Mesh,
}

/// What a reader returns.
#[derive(Debug, Default)]
pub struct Loaded {
    pub objects: Vec<Object>,
    /// Objects in the file before any merging.
    pub object_count: usize,
    /// True when the file has several objects that `merge` would join (OBJ only).
    pub splittable: bool,
    pub warnings: Vec<String>,
}

impl Loaded {
    pub fn single(mesh: Mesh) -> Loaded {
        Loaded {
            objects: vec![Object {
                name: None,
                matrix: None,
                mesh,
            }],
            object_count: 1,
            ..Loaded::default()
        }
    }
}

#[derive(Debug)]
pub struct ImportedPart {
    pub name: String,
    pub matrix: Option<Matrix>,
    pub mesh: Mesh,
}

#[derive(Debug)]
pub struct Imported {
    pub format: ImportFormat,
    pub object_count: usize,
    pub splittable: bool,
    pub warnings: Vec<String>,
    pub parts: Vec<ImportedPart>,
}

pub const SUPPORTED: &str = "tricky3D opens STL, OBJ, PLY, 3MF, AMF and OFF files";

/// Reads a mesh file into welded, named parts. `merge` joins an OBJ's objects into one part.
pub fn import_file(path: &Path, merge: bool, progress: &mut Progress) -> Result<Imported, String> {
    let file = display_name(path);
    let Some(format) = ImportFormat::from_path(path) else {
        return Err(format!("Could not open \"{file}\": {SUPPORTED}."));
    };
    let loaded = read_format(path, format, merge, progress)
        .map_err(|reason| format!("Could not read \"{file}\": {reason}."))?;
    let Loaded {
        objects,
        object_count,
        splittable,
        warnings,
    } = loaded;
    let objects: Vec<Object> = objects
        .into_iter()
        .filter(|o| !o.mesh.indices.is_empty())
        .collect();
    if objects.is_empty() {
        return Err(format!("Could not read \"{file}\": it has no triangles."));
    }
    let names = part_names(&file_stem(path), objects.iter().map(|o| o.name.as_deref()));
    let parts = objects
        .into_iter()
        .zip(names)
        .map(|(o, name)| ImportedPart {
            name,
            matrix: o.matrix.filter(|m| *m != crate::mesh::IDENTITY),
            mesh: o.mesh,
        })
        .collect();
    Ok(Imported {
        format,
        object_count,
        splittable,
        warnings,
        parts,
    })
}

fn read_format(
    path: &Path,
    format: ImportFormat,
    merge: bool,
    progress: &mut Progress,
) -> Result<Loaded, String> {
    match format {
        // Binary STL streams from the file, so a 250 MB file is not held in memory twice.
        ImportFormat::Stl => stl::read_path(path, progress),
        ImportFormat::Obj => obj::read(&read_all(path, progress)?, merge, progress),
        ImportFormat::Ply => ply::read(&read_all(path, progress)?, progress),
        ImportFormat::ThreeMf => {
            threemf::read(&read_all(path, progress)?, progress).map(threemf::Package::into_loaded)
        }
        ImportFormat::Amf => amf::read(&read_all(path, progress)?, progress),
        ImportFormat::Off => off::read(&read_all(path, progress)?, progress),
    }
}

/// Part names: the file stem for a single part, "<stem> - <object name>" for several (falling
/// back to "Object <n>" for objects without a name).
pub fn part_names<'a>(
    stem: &str,
    names: impl ExactSizeIterator<Item = Option<&'a str>>,
) -> Vec<String> {
    if names.len() == 1 {
        return vec![stem.to_string()];
    }
    names
        .enumerate()
        .map(|(i, name)| match clean_name(name) {
            Some(name) => format!("{stem} - {name}"),
            None => format!("{stem} - Object {}", i + 1),
        })
        .collect()
}

/// A name from a file, trimmed and without control characters; None when nothing is left.
pub fn clean_name(name: Option<&str>) -> Option<String> {
    let cleaned: String = name?
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

pub fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Part".into())
}

/// Reads a whole file, reporting the read stage.
pub fn read_all(path: &Path, progress: &mut Progress) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|e| read_error(&e))?;
    let size = file.metadata().map_err(|e| read_error(&e))?.len();
    let size = usize::try_from(size).map_err(|_| "the file is too large".to_string())?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|_| "there is not enough memory to read it".to_string())?;
    const CHUNK: usize = 8 << 20;
    let mut chunk = vec![0u8; CHUNK.min(size.max(1))];
    loop {
        let n = file.read(&mut chunk).map_err(|e| read_error(&e))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        progress.report(Stage::Read, bytes.len() as f64 / size.max(1) as f64);
    }
    progress.report(Stage::Read, 1.0);
    Ok(bytes)
}

/// The reason part of a read error message.
pub fn read_error(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => "the file does not exist any more".into(),
        io::ErrorKind::PermissionDenied => "tricky3D is not allowed to read it".into(),
        io::ErrorKind::UnexpectedEof => "the file ends early".into(),
        io::ErrorKind::OutOfMemory => "there is not enough memory to read it".into(),
        _ => format!("the system reported an error ({e})"),
    }
}

/// The reason part of a write error message.
pub fn write_error(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => "the folder does not exist".into(),
        io::ErrorKind::PermissionDenied => {
            "tricky3D is not allowed to write there, or the file is open in another program".into()
        }
        io::ErrorKind::StorageFull => "the disk is full".into(),
        io::ErrorKind::OutOfMemory => "there is not enough memory".into(),
        _ => format!("the system reported an error ({e})"),
    }
}

/// Writes through a temporary file next to `path` and renames it into place, so a failed or
/// interrupted write never leaves a half-written file where the old one was. Two writes to the
/// same path at once each write their own whole file; the last rename wins.
pub fn write_atomic(
    path: &Path,
    write: impl FnOnce(BufWriter<File>) -> io::Result<BufWriter<File>>,
) -> io::Result<()> {
    let (temp, file) = create_temp(path)?;
    let result = (|| {
        let writer = write(BufWriter::with_capacity(1 << 20, file))?;
        let file = writer.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// A new file next to `path` with a name no other write uses. The name is short, so a file name
/// near the 255-character limit still fits, and `create_new` never opens a file (or a planted
/// link) that is already there: it moves on to the next name instead.
fn create_temp(path: &Path) -> io::Result<(PathBuf, File)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut attempts = 0;
    loop {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let temp = path.with_file_name(format!(".tricky3d-{}-{n}.part", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => return Ok((temp, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && attempts < 1000 => attempts += 1,
            Err(e) => return Err(e),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Export

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    StlBinary,
    StlAscii,
    Obj,
    Ply,
    ThreeMf,
    Amf,
}

impl ExportFormat {
    /// Parses the `format` field of an export request (src/io/ipc.ts `ExportFormat`).
    pub fn parse(id: &str) -> Option<ExportFormat> {
        Some(match id {
            "stl-binary" => ExportFormat::StlBinary,
            "stl-ascii" => ExportFormat::StlAscii,
            "obj" => ExportFormat::Obj,
            "ply" => ExportFormat::Ply,
            "3mf" => ExportFormat::ThreeMf,
            "amf" => ExportFormat::Amf,
            _ => return None,
        })
    }

    /// The extension files of this format must have.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::StlBinary | ExportFormat::StlAscii => "stl",
            ExportFormat::Obj => "obj",
            ExportFormat::Ply => "ply",
            ExportFormat::ThreeMf => "3mf",
            ExportFormat::Amf => "amf",
        }
    }
}

/// True when the path ends in `.ext` (any case).
pub fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// A part going to a file, with its transform already baked into the mesh.
pub struct NamedMesh<'a> {
    pub name: &'a str,
    pub mesh: &'a Mesh,
}

/// Writes `parts` into one file of `format`. STL and PLY merge the parts into one mesh; OBJ, 3MF
/// and AMF keep one object per part. `title` labels the whole file where the format has a slot.
pub fn write_export<W: Write + Seek>(
    w: W,
    format: ExportFormat,
    title: &str,
    parts: &[NamedMesh],
) -> io::Result<W> {
    match format {
        ExportFormat::StlBinary => stl::write_binary(w, parts),
        ExportFormat::StlAscii => stl::write_ascii(w, title, parts),
        ExportFormat::Obj => obj::write(w, parts),
        ExportFormat::Ply => ply::write(w, parts),
        ExportFormat::ThreeMf => threemf::write_export(w, parts),
        ExportFormat::Amf => amf::write(w, parts),
    }
}

/// Exports to `path` with a message naming the file on failure.
pub fn export_file(path: &Path, format: ExportFormat, parts: &[NamedMesh]) -> Result<(), String> {
    let file = display_name(path);
    let fail = |reason: String| format!("Could not write \"{file}\": {reason}.");
    // Commands take paths from the window; only ever write mesh files, never anything else.
    if !has_extension(path, format.extension()) {
        return Err(fail(format!(
            "the file name must end in .{}",
            format.extension()
        )));
    }
    let triangles: usize = parts.iter().map(|p| p.mesh.triangle_count()).sum();
    if triangles == 0 {
        return Err(fail("there are no triangles to export".into()));
    }
    if matches!(format, ExportFormat::StlBinary) && triangles > u32::MAX as usize {
        return Err(fail("binary STL cannot hold this many triangles".into()));
    }
    let title = file_stem(path);
    write_atomic(path, |w| write_export(w, format, &title, parts))
        .map_err(|e| fail(write_error(&e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_part_count() {
        assert_eq!(part_names("cube", [Some("A")].into_iter()), vec!["cube"]);
        assert_eq!(
            part_names("set", [Some(" A "), None, Some("")].into_iter()),
            vec!["set - A", "set - Object 2", "set - Object 3"]
        );
    }

    #[test]
    fn unknown_extensions_name_the_supported_formats() {
        let err = import_file(Path::new("model.dae"), false, &mut Progress::silent()).unwrap_err();
        assert_eq!(
            err,
            "Could not open \"model.dae\": tricky3D opens STL, OBJ, PLY, 3MF, AMF and OFF files."
        );
    }

    /// The text writers rely on Rust's `Display` for floats giving the shortest text that reads
    /// back to the same bits.
    #[test]
    fn float_display_is_shortest_round_trip() {
        for v in [
            0.1f32,
            1.0 / 3.0,
            25.4,
            -0.0,
            1e-30,
            3.402_823_5e38,
            123_456.79,
        ] {
            let text = v.to_string();
            assert_eq!(
                text.parse::<f32>().unwrap().to_bits(),
                v.to_bits(),
                "{text}"
            );
        }
        assert_eq!(0.1f32.to_string(), "0.1");
        assert_eq!(20f32.to_string(), "20");
    }
}
