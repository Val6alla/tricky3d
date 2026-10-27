//! Small helpers shared by the XML-based formats (3MF, AMF): text collection, number parsing,
//! escaping, length units and zip access.

use std::borrow::Cow;
use std::io::{Cursor, Read};

use quick_xml::XmlVersion;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesStart, Event};
use zip::ZipArchive;

use crate::mesh::fmt_count;

/// Appends the character data of `event` (text, CDATA or an entity reference) to `out`.
pub fn append_text(out: &mut String, event: &Event) {
    match event {
        Event::Text(t) => out.push_str(&t.xml10_content()),
        Event::CData(t) => out.push_str(&t.xml10_content()),
        Event::GeneralRef(r) => match r.resolve_char_ref() {
            Ok(Some(ch)) => out.push(ch),
            _ => match resolve_predefined_entity(r) {
                Some(text) => out.push_str(text),
                None => {
                    out.push('&');
                    out.push_str(r);
                    out.push(';');
                }
            },
        },
        _ => {}
    }
}

/// An unprefixed attribute's value, with entity references resolved.
pub fn attribute(e: &BytesStart, name: &str) -> Option<String> {
    let attr = e.attributes().flatten().find(|a| a.key.as_ref() == name)?;
    let value = attr
        .normalized_value(XmlVersion::Implicit1_0)
        .map(Cow::into_owned);
    Some(value.unwrap_or_else(|_| attr.value.into_owned()))
}

/// A finite number from XML text; None for anything else.
pub fn number(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// A vertex index from XML text.
pub fn index(text: &str) -> Option<u32> {
    text.trim().parse::<u32>().ok()
}

/// Escapes text for an XML attribute value or element content, dropping characters XML 1.0
/// cannot hold at all (control characters other than tab and line breaks).
pub fn escape(text: &str) -> Cow<'_, str> {
    if !text.chars().any(|c| {
        matches!(c, '&' | '<' | '>' | '"' | '\'')
            || (c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
    }) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 16);
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// Millimetres per unit for the unit names 3MF and AMF use; None for an unknown unit.
pub fn unit_scale(unit: &str) -> Option<f64> {
    Some(match unit.trim().to_ascii_lowercase().as_str() {
        "micron" | "micrometer" | "micrometre" => 0.001,
        "millimeter" | "millimetre" | "mm" => 1.0,
        "centimeter" | "centimetre" | "cm" => 10.0,
        "inch" | "in" => 25.4,
        "foot" | "feet" | "ft" => 304.8,
        "meter" | "metre" | "m" => 1000.0,
        _ => return None,
    })
}

/// The reason part of a message for XML that does not parse.
pub fn xml_error(what: &str, e: &quick_xml::Error) -> String {
    format!("its {what} is not valid XML ({e})")
}

/// Strips a UTF-8 byte order mark and checks the text is UTF-8.
pub fn utf8<'a>(bytes: &'a [u8], what: &str) -> Result<&'a str, String> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    std::str::from_utf8(bytes).map_err(|_| format!("its {what} is not UTF-8 text"))
}

/// How much one file may ask tricky3D to hold: ten times the 5-million-triangle files it is
/// built for. 3MF components can repeat a mesh exponentially (A uses B twice, B uses C twice,
/// ...), so a file of a few hundred bytes could otherwise ask for terabytes and abort the app.
pub const MAX_VISITS: u64 = 1_000_000;
pub const MAX_TRIANGLES: u64 = 50_000_000;
pub const MAX_VERTICES: u64 = 100_000_000;

/// Running totals checked against the limits as they grow, so a file over a limit stops at the
/// element that crosses it instead of after it has filled memory.
#[derive(Default)]
pub struct Budget {
    pub visits: u64,
    pub triangles: u64,
    pub vertices: u64,
}

impl Budget {
    /// More uses of objects (3MF components or build items).
    pub fn visit(&mut self, uses: u64) -> Result<(), String> {
        self.visits = self.visits.saturating_add(uses);
        if self.visits > MAX_VISITS {
            return Err("it repeats its components too many times".into());
        }
        Ok(())
    }

    /// Adds vertices and triangles; `verb` says how the file holds them ("has" while it is read,
    /// "would have" once its components are repeated).
    pub fn add(&mut self, vertices: u64, triangles: u64, verb: &str) -> Result<(), String> {
        self.vertices = self.vertices.saturating_add(vertices);
        self.triangles = self.triangles.saturating_add(triangles);
        let over = |limit: u64, what: &str| {
            Err(format!(
                "it {verb} more than {} {what}, which is more than tricky3D can handle",
                fmt_count(limit)
            ))
        };
        if self.triangles > MAX_TRIANGLES {
            return over(MAX_TRIANGLES, "triangles");
        }
        if self.vertices > MAX_VERTICES {
            return over(MAX_VERTICES, "vertices");
        }
        Ok(())
    }
}

pub type Zip<'a> = ZipArchive<Cursor<&'a [u8]>>;

pub fn open_zip(bytes: &[u8]) -> Result<Zip<'_>, String> {
    ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("it is not a valid zip package ({e})"))
}

/// Largest single entry tricky3D unpacks.
const MAX_ENTRY: u64 = 4 << 30;
/// Model XML packs to between a third and a twentieth of its size, a project of many copies of one
/// small part to about 1/150. Above `RATIO_FREE`, an entry may unpack to at most `MAX_RATIO` times
/// its packed size, so a zip bomb (a small file that unpacks to gigabytes of repeated text) is
/// refused before anything is unpacked; below it, the parse-time vertex and triangle limits apply.
const MAX_RATIO: u64 = 100;
const RATIO_FREE: u64 = 512 << 20;

/// The entry's actual name, matched without regard to case or a leading slash (OPC part names
/// are case-insensitive and 3MF targets start with "/").
pub fn find_entry(zip: &Zip, name: &str) -> Option<String> {
    let wanted = name.trim_start_matches('/');
    zip.file_names()
        .filter_map(Result::ok)
        .find(|n| n.trim_start_matches('/').eq_ignore_ascii_case(wanted))
        .map(|n| n.into_owned())
}

/// Names and unpacked sizes of the entries whose name ends with `suffix` (case-insensitive).
pub fn entries_ending(zip: &mut Zip, suffix: &str) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    for i in 0..zip.len() {
        if let Ok(entry) = zip.by_index(i) {
            let name = entry.name().map(|n| n.into_owned()).unwrap_or_default();
            if name.to_ascii_lowercase().ends_with(suffix) && !entry.is_dir() {
                out.push((name, entry.size()));
            }
        }
    }
    out
}

/// Most an entry packed into `packed` bytes may unpack to.
fn unpack_limit(packed: u64) -> u64 {
    packed
        .saturating_mul(MAX_RATIO)
        .clamp(RATIO_FREE, MAX_ENTRY)
}

/// Unpacks one entry into memory.
pub fn read_entry(zip: &mut Zip, name: &str) -> Result<Vec<u8>, String> {
    // Entry data lies before the central directory, which bounds a packed size that lies.
    let archive = zip.central_directory_start();
    let mut entry = zip
        .by_name(name)
        .map_err(|e| format!("its part \"{name}\" cannot be opened ({e})"))?;
    // zip fails a read past the declared size, so checking that size is enough.
    let size = entry.size();
    let packed = entry.compressed_size().min(archive);
    let limit = unpack_limit(packed);
    let too_large = |size: u64| {
        let mb = |bytes: u64| fmt_count(bytes.div_ceil(1 << 20));
        format!(
            "its part \"{name}\" unpacks to {} MB from {} MB, which is more than tricky3D can handle",
            mb(size),
            mb(packed)
        )
    };
    if size > limit {
        return Err(too_large(size));
    }
    let mut out = Vec::new();
    out.try_reserve_exact(size as usize)
        .map_err(|_| "there is not enough memory to unpack it".to_string())?;
    (&mut entry)
        .take(limit + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("its part \"{name}\" is damaged ({e})"))?;
    if out.len() as u64 > limit {
        return Err(too_large(out.len() as u64));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn unpack_limit_allows_well_packed_projects_and_refuses_bombs() {
        use super::{MAX_ENTRY, unpack_limit};
        const MB: u64 = 1 << 20;
        // A small file may unpack to 512 MB whatever its ratio: a project of many copies of one part.
        assert_eq!(unpack_limit(400 * 1024), 512 * MB);
        assert_eq!(unpack_limit(0), 512 * MB);
        // Past that, at most 100 times the packed size: a 23 MB file never unpacks to 4 GB.
        assert_eq!(unpack_limit(23 * MB), 2300 * MB);
        assert_eq!(unpack_limit(u64::MAX), MAX_ENTRY);
    }

    use super::*;

    #[test]
    fn escapes_markup_and_drops_control_characters() {
        assert_eq!(escape("plain"), "plain");
        assert_eq!(escape(r#"a<b>&"c'"#), "a&lt;b&gt;&amp;&quot;c&apos;");
        assert_eq!(escape("bell\u{7}"), "bell");
    }

    #[test]
    fn attribute_entities_are_resolved() {
        let mut reader =
            quick_xml::reader::NsReader::from_str(r#"<object name="A &amp; B &#x41;&#66;"/>"#);
        let Ok(Event::Empty(e)) = reader.read_event() else {
            panic!("expected an element")
        };
        assert_eq!(attribute(&e, "name").unwrap(), "A & B AB");
        assert_eq!(attribute(&e, "id"), None);
    }

    #[test]
    fn units() {
        assert_eq!(unit_scale("inch"), Some(25.4));
        assert_eq!(unit_scale("Feet"), Some(304.8));
        assert_eq!(unit_scale("parsec"), None);
    }
}
