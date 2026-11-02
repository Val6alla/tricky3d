//! Mesh file formats: detection, reading into welded meshes, and writing. Readers return a reason
//! (a lower-case clause such as "the file ends early") and `import_file` / `export_file` turn it
//! into the full sentence the user sees, naming the file.

pub mod amf;
pub mod gts;
pub mod obj;
pub mod off;
pub mod ply;
pub mod scene;
pub mod stl;
pub mod tds;
pub mod threemf;
pub mod vrml;
pub mod x3d;
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
    /// VRML 2.0 / VRML97, and X3D's classic text encoding.
    Vrml,
    X3d,
    Tds,
    Gts,
}

impl ImportFormat {
    /// Every format tricky3D reads, in menu order.
    pub const ALL: [ImportFormat; 10] = [
        ImportFormat::Stl,
        ImportFormat::Obj,
        ImportFormat::Ply,
        ImportFormat::ThreeMf,
        ImportFormat::Amf,
        ImportFormat::Off,
        ImportFormat::Vrml,
        ImportFormat::X3d,
        ImportFormat::Tds,
        ImportFormat::Gts,
    ];

    /// The format for `path`'s extension, in any letter case.
    pub fn from_path(path: &Path) -> Option<ImportFormat> {
        let ext = path.extension()?.to_str()?;
        ImportFormat::ALL
            .into_iter()
            .find(|f| f.extensions().iter().any(|e| e.eq_ignore_ascii_case(ext)))
    }

    /// The `format` field of the import result (src/io/ipc.ts `ImportFormat`), which is also the
    /// format's main file extension.
    pub fn id(self) -> &'static str {
        match self {
            ImportFormat::Stl => "stl",
            ImportFormat::Obj => "obj",
            ImportFormat::Ply => "ply",
            ImportFormat::ThreeMf => "3mf",
            ImportFormat::Amf => "amf",
            ImportFormat::Off => "off",
            ImportFormat::Vrml => "wrl",
            ImportFormat::X3d => "x3d",
            ImportFormat::Tds => "3ds",
            ImportFormat::Gts => "gts",
        }
    }

    /// Every extension files of the format have.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            ImportFormat::Vrml => &["wrl", "vrml", "wrz", "x3dv", "x3dvz"],
            ImportFormat::X3d => &["x3d", "x3dz"],
            ImportFormat::Stl => &["stl"],
            ImportFormat::Obj => &["obj"],
            ImportFormat::Ply => &["ply"],
            ImportFormat::ThreeMf => &["3mf"],
            ImportFormat::Amf => &["amf"],
            ImportFormat::Off => &["off"],
            ImportFormat::Tds => &["3ds"],
            ImportFormat::Gts => &["gts"],
        }
    }

    /// The name in the open dialog's format menu.
    pub fn label(self) -> &'static str {
        match self {
            ImportFormat::Stl => "STL",
            ImportFormat::Obj => "OBJ",
            ImportFormat::Ply => "PLY",
            ImportFormat::ThreeMf => "3MF",
            ImportFormat::Amf => "AMF",
            ImportFormat::Off => "OFF",
            ImportFormat::Vrml => "VRML",
            ImportFormat::X3d => "X3D",
            ImportFormat::Tds => "3DS",
            ImportFormat::Gts => "GTS",
        }
    }
}

/// How a file is read: several objects merged into one part or not, and its unit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ImportOptions {
    /// Join a file's objects into one part.
    pub merge: bool,
    /// Millimetres per unit of the file's numbers, whatever the file says; None reads the unit
    /// the file names, else millimetres.
    pub units: Option<f64>,
}

/// One object a reader found, already welded, before it gets its part name.
#[derive(Debug, Default)]
pub struct Object {
    /// The name the file gives the object, if any.
    pub name: Option<String>,
    /// Placement from the file (3MF build items); None means identity.
    pub matrix: Option<Matrix>,
    pub mesh: Mesh,
    /// The object's colour (sRGB), where the file gives one.
    pub color: Option<[u8; 3]>,
    /// A colour per triangle (r, g, b), where the file gives several.
    pub colors: Option<Vec<u8>>,
}

/// What a reader returns.
#[derive(Debug, Default)]
pub struct Loaded {
    pub objects: Vec<Object>,
    /// Objects in the file before any merging.
    pub object_count: usize,
    /// True when the file has several objects that `merge` would join.
    pub splittable: bool,
    pub warnings: Vec<String>,
    /// Millimetres per unit, when the file names its unit (3MF and AMF always do: millimetres
    /// unless they say otherwise; X3D with a UNIT statement). The reader has applied it.
    pub unit: Option<f64>,
}

impl Loaded {
    pub fn single(mesh: Mesh) -> Loaded {
        Loaded {
            objects: vec![Object {
                mesh,
                ..Object::default()
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
    pub color: Option<[u8; 3]>,
    pub colors: Option<Vec<u8>>,
}

#[derive(Debug)]
pub struct Imported {
    pub format: ImportFormat,
    pub object_count: usize,
    pub splittable: bool,
    pub warnings: Vec<String>,
    pub parts: Vec<ImportedPart>,
    /// Millimetres per unit the numbers were read in: the one asked for, else the file's own (None
    /// when it names none and was read as millimetres).
    pub unit: Option<f64>,
}

pub const SUPPORTED: &str =
    "tricky3D opens STL, OBJ, PLY, 3MF, AMF, OFF, VRML, X3D, 3DS and GTS files";

/// Reads a mesh file into welded, named parts. `merge` joins the file's objects into one part.
pub fn import_file(path: &Path, merge: bool, progress: &mut Progress) -> Result<Imported, String> {
    import_with(path, &ImportOptions { merge, units: None }, progress)
}

/// Reads a mesh file into welded, named parts, as `options` say.
pub fn import_with(
    path: &Path,
    options: &ImportOptions,
    progress: &mut Progress,
) -> Result<Imported, String> {
    let file = display_name(path);
    let Some(format) = ImportFormat::from_path(path) else {
        return Err(format!("Could not open \"{file}\": {SUPPORTED}."));
    };
    let fail = |reason: String| format!("Could not read \"{file}\": {reason}.");
    let loaded = read_format(path, format, options.merge, progress).map_err(fail)?;
    let Loaded {
        objects,
        object_count,
        warnings,
        unit,
        ..
    } = loaded;
    let mut objects: Vec<Object> = objects
        .into_iter()
        .filter(|o| !o.mesh.indices.is_empty())
        .collect();
    if objects.is_empty() {
        return Err(fail("it has no triangles".into()));
    }
    let splittable = objects.len() > 1 && !options.merge;
    if options.merge && objects.len() > 1 {
        objects = vec![merged(objects).map_err(fail)?];
    }
    // A unit asked for replaces the one the reader applied.
    let unit = match options.units {
        Some(asked) if asked.is_finite() && asked > 0.0 => {
            let factor = asked / unit.unwrap_or(1.0);
            if factor != 1.0 {
                for o in &mut objects {
                    rescale(o, factor);
                }
            }
            Some(asked)
        }
        _ => unit,
    };
    let names = part_names(&file_stem(path), objects.iter().map(|o| o.name.as_deref()));
    let parts = objects
        .into_iter()
        .zip(names)
        .map(|(o, name)| ImportedPart {
            name,
            matrix: o.matrix.filter(|m| *m != crate::mesh::IDENTITY),
            mesh: o.mesh,
            color: o.color,
            colors: o.colors,
        })
        .collect();
    Ok(Imported {
        format,
        object_count,
        splittable,
        warnings,
        parts,
        unit,
    })
}

/// The objects as one: each placed by its matrix, side by side in one mesh (not welded together,
/// so objects that touch stay separate shells), with a colour per triangle when they had colours.
fn merged(objects: Vec<Object>) -> Result<Object, String> {
    let baked: Vec<Mesh> = objects
        .iter()
        .map(|o| match &o.matrix {
            Some(m) => crate::mesh::bake(&o.mesh, m),
            None => o.mesh.clone(),
        })
        .collect();
    let refs: Vec<&Mesh> = baked.iter().collect();
    let mesh = crate::ops::concat(&refs)?;
    let coloured = objects
        .iter()
        .any(|o| o.color.is_some() || o.colors.is_some());
    let colors = coloured.then(|| {
        let mut out = Vec::with_capacity(mesh.indices.len());
        for o in &objects {
            match &o.colors {
                Some(c) => out.extend_from_slice(c),
                None => {
                    let c = o.color.unwrap_or(DEFAULT_COLOR);
                    for _ in 0..o.mesh.triangle_count() {
                        out.extend_from_slice(&c);
                    }
                }
            }
        }
        out
    });
    let color = objects.iter().find_map(|o| o.color);
    Ok(Object {
        name: None,
        matrix: None,
        mesh,
        color,
        colors,
    })
}

/// The colour a triangle gets in a part with colours per triangle when its own object had none.
const DEFAULT_COLOR: [u8; 3] = [204, 204, 204];

/// Every length of the object times `factor`: its points, and its matrix's translation.
fn rescale(o: &mut Object, factor: f64) {
    for x in &mut o.mesh.positions {
        *x = (*x as f64 * factor) as f32;
    }
    if let Some(m) = &mut o.matrix {
        for x in &mut m[12..15] {
            *x *= factor;
        }
    }
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
        ImportFormat::Vrml => vrml::read(&read_all(path, progress)?, progress),
        ImportFormat::X3d => x3d::read(&read_all(path, progress)?, progress),
        ImportFormat::Tds => tds::read(&read_all(path, progress)?, progress),
        ImportFormat::Gts => gts::read(&read_all(path, progress)?, progress),
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
    Off,
    Vrml,
    X3d,
    Tds,
    Gts,
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
            "off" => ExportFormat::Off,
            "wrl" => ExportFormat::Vrml,
            "x3d" => ExportFormat::X3d,
            "3ds" => ExportFormat::Tds,
            "gts" => ExportFormat::Gts,
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
            ExportFormat::Off => "off",
            ExportFormat::Vrml => "wrl",
            ExportFormat::X3d => "x3d",
            ExportFormat::Tds => "3ds",
            ExportFormat::Gts => "gts",
        }
    }
}

/// True when the path ends in `.ext` (any case).
pub fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// `name` as a file name every system accepts, as `sanitizeFileName` in src/io/files.ts: characters
/// Windows, macOS or Linux forbid become "_", trailing dots and spaces go (Windows drops them), a
/// device name such as "CON" gets a "_", and an empty name becomes "Part".
pub fn safe_file_name(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut out: String = replaced.trim().chars().take(200).collect();
    out.truncate(out.trim_end_matches(['.', ' ']).len());
    let lower = out.to_ascii_lowercase();
    let numbered = |prefix: &str| {
        lower
            .strip_prefix(prefix)
            .is_some_and(|n| n.len() == 1 && matches!(n.as_bytes()[0], b'1'..=b'9'))
    };
    if ["con", "prn", "aux", "nul"].contains(&lower.as_str()) || numbered("com") || numbered("lpt")
    {
        out.push('_');
    }
    if out.is_empty() { "Part".into() } else { out }
}

/// The files of an export with one file per part, in part order: each part's name made safe
/// (`safe_file_name`) with `.ext`. Names that would be the same file, in any letter case as on
/// Windows and macOS, get " (2)", " (3)"… so no part overwrites another.
pub fn part_file_names<'a>(names: impl IntoIterator<Item = &'a str>, ext: &str) -> Vec<String> {
    let mut taken = std::collections::HashSet::new();
    names
        .into_iter()
        .map(|name| {
            let base = safe_file_name(name);
            (1..)
                .map(|n| match n {
                    1 => format!("{base}.{ext}"),
                    n => format!("{base} ({n}).{ext}"),
                })
                .find(|file| taken.insert(file.to_lowercase()))
                .expect("a free name")
        })
        .collect()
}

/// A part going to a file, with its transform already baked into the mesh.
pub struct NamedMesh<'a> {
    pub name: &'a str,
    pub mesh: &'a Mesh,
}

/// Writes `parts` into one file of `format`, each where it sits on the platform: the callers bake
/// every part's matrix into its vertices first, and no writer moves anything. STL, PLY, OFF and GTS
/// merge the parts into one mesh; OBJ, 3MF, AMF, VRML, X3D and 3DS keep one object per part. `title` labels the whole file where the format has a slot.
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
        ExportFormat::Off => off::write(w, parts),
        ExportFormat::Vrml => vrml::write(w, parts),
        ExportFormat::X3d => x3d::write(w, parts),
        ExportFormat::Tds => tds::write(w, parts),
        ExportFormat::Gts => gts::write(w, parts),
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
            "Could not open \"model.dae\": tricky3D opens STL, OBJ, PLY, 3MF, AMF, OFF, VRML, X3D, \
             3DS and GTS files."
        );
    }

    #[test]
    fn file_names_are_safe_on_every_system() {
        assert_eq!(
            safe_file_name("gun - barrel (cut 1)"),
            "gun - barrel (cut 1)"
        );
        assert_eq!(
            safe_file_name(r#"a/b\c:d*e?f"g<h>i|j"#),
            "a_b_c_d_e_f_g_h_i_j"
        );
        assert_eq!(safe_file_name("  bracket.. "), "bracket");
        assert_eq!(safe_file_name("tab\there"), "tab_here");
        assert_eq!(safe_file_name("CON"), "CON_");
        assert_eq!(safe_file_name("com7"), "com7_");
        assert_eq!(safe_file_name("com10"), "com10");
        assert_eq!(safe_file_name("console"), "console");
        assert_eq!(safe_file_name(" . "), "Part");
        assert_eq!(safe_file_name(""), "Part");
        assert_eq!(safe_file_name(&"x".repeat(300)).len(), 200);
    }

    #[test]
    fn one_file_per_part_never_gives_two_parts_the_same_file() {
        assert_eq!(
            part_file_names(
                ["gun - barrel", "Stand", "stand", "stand (2)", "a/b", "a_b"],
                "stl"
            ),
            [
                "gun - barrel.stl",
                "Stand.stl",
                "stand (2).stl",
                "stand (2) (2).stl",
                "a_b.stl",
                "a_b (2).stl"
            ]
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
