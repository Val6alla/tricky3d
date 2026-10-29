//! Text for labels: the bundled Noto Sans, which every computer has because it is built in, and
//! the fonts installed on the computer, read with ttf-parser. A caption becomes contours, its
//! glyphs' outlines with their curves cut into straight pieces, each line centred under the one
//! before.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Serialize;
use ttf_parser::{Face, OutlineBuilder};

use super::region::P2;

/// The id of the bundled font.
pub const BUNDLED_ID: &str = "bundled";
/// Noto Sans Regular, under the SIL Open Font License (src-tauri/fonts/NotoSans-OFL.txt).
pub(crate) static BUNDLED: &[u8] = include_bytes!("../../fonts/NotoSans-Regular.ttf");

/// A font the window can offer. `id` is what it sends back; `family` and `style` are its names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FontInfo {
    pub id: String,
    pub family: String,
    pub style: String,
    pub bundled: bool,
}

/// Where a font's outlines are: the bundled data, or a face of a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontSource {
    pub path: Option<PathBuf>,
    pub index: u32,
}

/// The folders fonts are installed in on this computer: the system's and the user's.
pub fn system_dirs() -> Vec<PathBuf> {
    let env = |key: &str| std::env::var_os(key).map(PathBuf::from);
    let mut dirs = Vec::new();
    if cfg!(windows) {
        dirs.extend(env("WINDIR").map(|w| w.join("Fonts")));
        dirs.extend(env("LOCALAPPDATA").map(|l| l.join("Microsoft").join("Windows").join("Fonts")));
    } else if cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/System/Library/Fonts"));
        dirs.push(PathBuf::from("/Library/Fonts"));
        dirs.extend(env("HOME").map(|h| h.join("Library").join("Fonts")));
    } else {
        dirs.push(PathBuf::from("/usr/share/fonts"));
        dirs.push(PathBuf::from("/usr/local/share/fonts"));
        dirs.extend(env("HOME").map(|h| h.join(".local").join("share").join("fonts")));
        dirs.extend(env("HOME").map(|h| h.join(".fonts")));
    }
    dirs
}

/// The bundled font first, then every face of every font file in `dirs` (looked through up to
/// four folders deep), sorted by family and style. Files that cannot be read are left out.
pub fn list(dirs: &[PathBuf]) -> Vec<(FontInfo, FontSource)> {
    let mut found = Vec::new();
    for dir in dirs {
        collect(dir, 0, &mut found);
    }
    let mut fonts: Vec<(FontInfo, FontSource)> = Vec::new();
    for path in found {
        for (index, family, style) in faces_of(&path) {
            let id = format!("{}#{index}", path.display());
            let source = FontSource {
                path: Some(path.clone()),
                index,
            };
            fonts.push((
                FontInfo {
                    id,
                    family,
                    style,
                    bundled: false,
                },
                source,
            ));
        }
    }
    fonts.sort_by(|a, b| {
        (
            a.0.family.to_lowercase(),
            style_rank(&a.0.style),
            &a.0.style,
        )
            .cmp(&(
                b.0.family.to_lowercase(),
                style_rank(&b.0.style),
                &b.0.style,
            ))
    });
    // The same family and style twice (a user copy of a system font): keep the first.
    fonts.dedup_by(|a, b| a.0.family == b.0.family && a.0.style == b.0.style);
    let bundled = (
        FontInfo {
            id: BUNDLED_ID.into(),
            family: "Noto Sans (built in)".into(),
            style: "Regular".into(),
            bundled: true,
        },
        FontSource {
            path: None,
            index: 0,
        },
    );
    std::iter::once(bundled).chain(fonts).collect()
}

/// Regular first, then bold, italic and the rest, within a family.
fn style_rank(style: &str) -> u8 {
    match style.to_lowercase().as_str() {
        "regular" | "normal" | "book" | "roman" => 0,
        "bold" => 1,
        "italic" | "oblique" => 2,
        "bold italic" | "bold oblique" => 3,
        _ => 4,
    }
}

fn collect(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth < 4 {
                collect(&path, depth + 1, out);
            }
        } else if path.extension().is_some_and(|e| {
            ["ttf", "otf", "ttc", "otc"].contains(&e.to_string_lossy().to_lowercase().as_str())
        }) {
            out.push(path);
        }
    }
}

/// Each face in a font file with its family and style names. Only the file's table directories
/// and name tables are read, so a large font costs no more than a small one.
fn faces_of(path: &Path) -> Vec<(u32, String, String)> {
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let mut read = |offset: u64, len: usize| -> Option<Vec<u8>> {
        let mut buf = vec![0; len];
        file.seek(SeekFrom::Start(offset)).ok()?;
        file.read_exact(&mut buf).ok()?;
        Some(buf)
    };
    let Some(head) = read(0, 12) else {
        return Vec::new();
    };
    let starts: Vec<u64> = if &head[0..4] == b"ttcf" {
        let count = u32::from_be_bytes([head[8], head[9], head[10], head[11]]).min(64) as usize;
        let Some(offsets) = read(12, 4 * count) else {
            return Vec::new();
        };
        offsets
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&c| u32::from_be_bytes(c) as u64)
            .collect()
    } else {
        vec![0]
    };
    let mut faces = Vec::new();
    for (index, start) in starts.into_iter().enumerate() {
        let Some(dir) = read(start, 12) else { continue };
        let tables = u16::from_be_bytes([dir[4], dir[5]]) as usize;
        let Some(records) = read(start + 12, 16 * tables) else {
            continue;
        };
        // Only fonts with outlines a label can use (TrueType or CFF), not bitmap-only fonts.
        let records = records.as_chunks::<16>().0;
        let has = |tag: &[u8]| records.iter().any(|r| &r[0..4] == tag);
        if !(has(b"glyf") || has(b"CFF ") || has(b"CFF2")) {
            continue;
        }
        let Some(name) = records.iter().find(|r| &r[0..4] == b"name") else {
            continue;
        };
        let offset = u32::from_be_bytes([name[8], name[9], name[10], name[11]]) as u64;
        let len = u32::from_be_bytes([name[12], name[13], name[14], name[15]]) as usize;
        if len > 4 << 20 {
            continue;
        }
        let Some(bytes) = read(offset, len) else {
            continue;
        };
        let Some(table) = ttf_parser::name::Table::parse(&bytes) else {
            continue;
        };
        if let Some((family, style)) = names(&table) {
            faces.push((index as u32, family, style));
        }
    }
    faces
}

/// The family and style names, typographic ones first (they group a family's many weights),
/// in English where the font has several languages.
fn names(table: &ttf_parser::name::Table) -> Option<(String, String)> {
    let find = |id: u16| -> Option<String> {
        let mut best: Option<(u8, String)> = None;
        for name in table.names {
            if name.name_id != id {
                continue;
            }
            let Some(text) = name.to_string() else {
                continue;
            };
            let rank = if name.language_id == 0x0409 { 0 } else { 1 };
            if best.as_ref().is_none_or(|b| rank < b.0) {
                best = Some((rank, text));
            }
        }
        best.map(|b| b.1).filter(|t| !t.trim().is_empty())
    };
    let family = find(16).or_else(|| find(1))?;
    let style = find(17)
        .or_else(|| find(2))
        .unwrap_or_else(|| "Regular".into());
    Some((family, style))
}

/// The font's bytes: the bundled data, or the file read whole.
pub fn load(source: &FontSource) -> Result<Vec<u8>, String> {
    match &source.path {
        None => Ok(BUNDLED.to_vec()),
        Some(path) => std::fs::read(path).map_err(|e| {
            format!(
                "Could not read the font {}: {e}.",
                path.file_name().map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().into_owned()
                )
            )
        }),
    }
}

/// The caption's glyph outlines in font units, y up, lines stacked downwards with the font's
/// line spacing and each line centred on x = 0. `size` (points) sets how finely curves are cut:
/// the outline stays within a fifth of a pixel of the curve at that size on a 96 dpi screen.
/// Fails with a sentence naming the letters the font does not have.
pub(crate) fn text_contours(
    data: &[u8],
    index: u32,
    text: &str,
    size: f64,
) -> Result<Vec<Vec<P2>>, String> {
    let face = Face::parse(data, index).map_err(|_| "The font could not be read.".to_string())?;
    let em = face.units_per_em() as f64;
    let pixels = size.max(1.0) * 96.0 / 72.0;
    let tolerance = 0.2 * em / pixels;
    let line_height =
        (face.ascender() as f64 - face.descender() as f64 + face.line_gap() as f64).max(em * 0.5);
    let mut missing: Vec<char> = Vec::new();
    let mut contours = Vec::new();
    for (row, line) in text.lines().enumerate() {
        let glyphs: Vec<_> = line
            .chars()
            .filter(|c| !c.is_control())
            .filter_map(|c| match face.glyph_index(c) {
                Some(g) => Some(g),
                None => {
                    if !c.is_whitespace() && !missing.contains(&c) {
                        missing.push(c);
                    }
                    None
                }
            })
            .collect();
        let advance = |g| face.glyph_hor_advance(g).unwrap_or(0) as f64;
        let width: f64 = glyphs.iter().map(|&g| advance(g)).sum();
        let mut x = -width / 2.0;
        let y = -(row as f64) * line_height;
        for g in glyphs {
            let mut pen = Pen {
                origin: [x, y],
                tolerance,
                contours: &mut contours,
                current: Vec::new(),
                last: [0.0; 2],
            };
            face.outline_glyph(g, &mut pen);
            pen.finish();
            x += advance(g);
        }
    }
    if !missing.is_empty() {
        let letters: String = missing.iter().take(8).collect();
        return Err(format!(
            "The font has no shape for {letters}. Pick another font in Font › …."
        ));
    }
    Ok(contours)
}

/// Collects one glyph's contours, shifted to its place in the line.
struct Pen<'a> {
    origin: P2,
    tolerance: f64,
    contours: &'a mut Vec<Vec<P2>>,
    current: Vec<P2>,
    last: P2,
}

impl Pen<'_> {
    fn at(&self, x: f32, y: f32) -> P2 {
        [self.origin[0] + x as f64, self.origin[1] + y as f64]
    }

    fn finish(&mut self) {
        let contour = std::mem::take(&mut self.current);
        if contour.len() >= 3 {
            self.contours.push(contour);
        }
    }

    /// Pieces for a curve whose control polygon bends by `bend` (font units).
    fn pieces(&self, bend: f64) -> usize {
        ((bend / (8.0 * self.tolerance)).sqrt().ceil() as usize).clamp(1, 64)
    }
}

impl OutlineBuilder for Pen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish();
        let p = self.at(x, y);
        self.current.push(p);
        self.last = p;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.current.push(p);
        self.last = p;
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (p0, p1, p2) = (self.last, self.at(x1, y1), self.at(x, y));
        let bend = (p0[0] - 2.0 * p1[0] + p2[0]).hypot(p0[1] - 2.0 * p1[1] + p2[1]);
        let n = self.pieces(bend);
        for k in 1..=n {
            let t = k as f64 / n as f64;
            let s = 1.0 - t;
            self.current.push([
                s * s * p0[0] + 2.0 * s * t * p1[0] + t * t * p2[0],
                s * s * p0[1] + 2.0 * s * t * p1[1] + t * t * p2[1],
            ]);
        }
        self.last = p2;
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (p0, p1, p2, p3) = (self.last, self.at(x1, y1), self.at(x2, y2), self.at(x, y));
        let b1 = (p0[0] - 2.0 * p1[0] + p2[0]).hypot(p0[1] - 2.0 * p1[1] + p2[1]);
        let b2 = (p1[0] - 2.0 * p2[0] + p3[0]).hypot(p1[1] - 2.0 * p2[1] + p3[1]);
        let n = self.pieces(6.0 * b1.max(b2));
        for k in 1..=n {
            let t = k as f64 / n as f64;
            let s = 1.0 - t;
            let (a, b, c, d) = (s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t);
            self.current.push([
                a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
                a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
            ]);
        }
        self.last = p3;
    }

    fn close(&mut self) {
        self.finish();
    }
}
