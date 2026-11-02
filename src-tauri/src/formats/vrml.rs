//! VRML 2.0 / VRML97 (.wrl, .vrml, gzip-compressed .wrz) and X3D's classic text encoding (.x3dv),
//! read into the scene graph of `scene`. Fields are read without a schema: numbers until the next
//! name, quoted strings, TRUE and FALSE, nodes, and lists of any of them in brackets. PROTO and
//! EXTERNPROTO definitions, ROUTEs and X3D's IMPORT and EXPORT statements are skipped; instances of
//! a PROTO give no geometry (a warning says so). X3D's UNIT statement sets the length unit; without
//! one the numbers are millimetres, as 3D printing tools write them (the standards say metres).
//! VRML 1.0, a different language, is refused with a sentence.
//!
//! Export writes VRML 2.0 with one Shape per part, its points and an IndexedFaceSet of triangles,
//! in millimetres.

use std::collections::HashMap;
use std::io::{self, Write};
use std::rc::Rc;

use super::scene::{self, Field, Node};
use super::{Loaded, NamedMesh, Object, clean_name};
use crate::mesh::weld_indexed;
use crate::progress::{Progress, Stage};

pub fn read(bytes: &[u8], progress: &mut Progress) -> Result<Loaded, String> {
    let unpacked;
    let bytes = if bytes.starts_with(&[0x1f, 0x8b]) {
        unpacked = super::xml::gunzip(bytes)?;
        &unpacked[..]
    } else {
        bytes
    };
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_start_matches('\u{feff}');
    let first = text.lines().next().unwrap_or_default().trim();
    let classic_x3d = first.starts_with("#X3D");
    if first.starts_with("#VRML V1.0") {
        return Err(
            "it is a VRML 1.0 file, which tricky3D cannot open (only VRML 2.0 and VRML97)".into(),
        );
    }
    if !first.starts_with("#VRML V2.0") && !classic_x3d {
        return Err("it is not a VRML 2.0 file (it does not start with \"#VRML V2.0\")".into());
    }
    let mut parser = Parser {
        lexer: Lexer::new(text),
        defs: HashMap::new(),
        protos: Vec::new(),
        unit: None,
        total: text.len().max(1),
        progress,
        reported: 0,
    };
    let roots = parser.statements(false)?;
    let mut warnings = Vec::new();
    if !parser.protos.is_empty() {
        warnings.push(format!(
            "Shapes made by the file's own node types ({}) were left out.",
            parser.protos.join(", ")
        ));
    }
    let scale = parser.unit.unwrap_or(1.0);
    let declared = parser.unit;
    from_scene(&roots, scale, declared, warnings, progress)
}

/// The objects of a scene, welded, as a reader returns them.
pub(crate) fn from_scene(
    roots: &[Rc<Node>],
    scale: f64,
    declared: Option<f64>,
    mut warnings: Vec<String>,
    progress: &mut Progress,
) -> Result<Loaded, String> {
    let objects = scene::collect(roots, scale, &mut warnings);
    let count = objects.len();
    let mut out = Vec::with_capacity(count);
    for (k, o) in objects.into_iter().enumerate() {
        let mesh = weld_indexed(&o.positions, &o.indices)?;
        out.push(Object {
            name: clean_name(o.name.as_deref()),
            matrix: None,
            mesh,
            color: o.color,
            colors: o.colors,
        });
        progress.report(Stage::Weld, (k + 1) as f64 / count.max(1) as f64);
    }
    Ok(Loaded {
        object_count: out.len(),
        splittable: out.len() > 1,
        objects: out,
        warnings,
        unit: declared,
    })
}

// ---------------------------------------------------------------------------------------------
// Tokens

#[derive(Clone, Copy, Debug, PartialEq)]
enum Token<'a> {
    Open,
    Close,
    OpenList,
    CloseList,
    Number(f64),
    Text(&'a str),
    Word(&'a str),
    End,
}

struct Lexer<'a> {
    text: &'a str,
    pos: usize,
    peeked: Option<(Token<'a>, usize)>,
}

impl<'a> Lexer<'a> {
    fn new(text: &'a str) -> Lexer<'a> {
        // The header line is a comment like any other.
        Lexer {
            text,
            pos: 0,
            peeked: None,
        }
    }

    fn skip_space(&mut self) {
        let b = self.text.as_bytes();
        while self.pos < b.len() {
            match b[self.pos] {
                b' ' | b'\t' | b'\r' | b'\n' | b',' => self.pos += 1,
                b'#' => {
                    while self.pos < b.len() && b[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
    }

    fn scan(&mut self) -> Token<'a> {
        self.skip_space();
        let b = self.text.as_bytes();
        if self.pos >= b.len() {
            return Token::End;
        }
        let start = self.pos;
        match b[start] {
            b'{' => {
                self.pos += 1;
                Token::Open
            }
            b'}' => {
                self.pos += 1;
                Token::Close
            }
            b'[' => {
                self.pos += 1;
                Token::OpenList
            }
            b']' => {
                self.pos += 1;
                Token::CloseList
            }
            b'"' => {
                self.pos += 1;
                while self.pos < b.len() && b[self.pos] != b'"' {
                    if b[self.pos] == b'\\' {
                        self.pos += 1;
                    }
                    self.pos += 1;
                }
                let end = self.pos.min(b.len());
                self.pos = (self.pos + 1).min(b.len());
                Token::Text(&self.text[start + 1..end])
            }
            _ => {
                while self.pos < b.len()
                    && !matches!(
                        b[self.pos],
                        b' ' | b'\t'
                            | b'\r'
                            | b'\n'
                            | b','
                            | b'{'
                            | b'}'
                            | b'['
                            | b']'
                            | b'"'
                            | b'#'
                    )
                {
                    self.pos += 1;
                }
                let word = &self.text[start..self.pos];
                let c = b[start];
                if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.' {
                    if let Ok(v) = word.parse::<f64>() {
                        return Token::Number(v);
                    }
                    if let Some(hex) = word.strip_prefix("0x").or_else(|| word.strip_prefix("0X"))
                        && let Ok(v) = i64::from_str_radix(hex, 16)
                    {
                        return Token::Number(v as f64);
                    }
                }
                Token::Word(word)
            }
        }
    }

    fn next(&mut self) -> Token<'a> {
        match self.peeked.take() {
            Some((t, pos)) => {
                self.pos = pos;
                t
            }
            None => self.scan(),
        }
    }

    fn peek(&mut self) -> Token<'a> {
        if let Some((t, _)) = self.peeked {
            return t;
        }
        let t = self.scan();
        self.peeked = Some((t, self.pos));
        t
    }
}

// ---------------------------------------------------------------------------------------------
// Parsing

struct Parser<'a, 'p, 'q> {
    lexer: Lexer<'a>,
    defs: HashMap<String, Rc<Node>>,
    /// Node types the file defines itself.
    protos: Vec<String>,
    /// mm per unit, from an X3D UNIT statement.
    unit: Option<f64>,
    total: usize,
    progress: &'p mut Progress<'q>,
    reported: usize,
}

/// Nested deeper than this, a file is refused rather than overflow the stack.
const MAX_NESTING: usize = 512;

impl Parser<'_, '_, '_> {
    fn report(&mut self) {
        let at = self.lexer.pos;
        if at - self.reported >= 1 << 20 {
            self.reported = at;
            self.progress
                .report(Stage::Parse, at as f64 / self.total as f64);
        }
    }

    fn bad(&self, what: &str) -> String {
        let line = self.lexer.text[..self.lexer.pos.min(self.lexer.text.len())]
            .matches('\n')
            .count()
            + 1;
        format!("its VRML data is not valid at line {line} ({what})")
    }

    /// Statements until the end, or until `}` when `in_body`.
    fn statements(&mut self, in_body: bool) -> Result<Vec<Rc<Node>>, String> {
        let mut out = Vec::new();
        loop {
            self.report();
            match self.lexer.peek() {
                Token::End => {
                    if in_body {
                        return Err(self.bad("a { is never closed"));
                    }
                    return Ok(out);
                }
                Token::Close if in_body => return Ok(out),
                Token::Word(w) => match w {
                    "PROTO" | "EXTERNPROTO" => self.skip_proto()?,
                    "ROUTE" => self.skip_words(4),
                    "IMPORT" | "EXPORT" => self.skip_statement(),
                    "PROFILE" | "COMPONENT" => self.skip_words(2),
                    "META" => self.skip_words(3),
                    "UNIT" => self.unit_statement(),
                    _ => {
                        if let Some(n) = self.node(0)? {
                            out.push(n);
                        }
                    }
                },
                _ => {
                    self.lexer.next();
                }
            }
        }
    }

    fn skip_words(&mut self, n: usize) {
        for _ in 0..n {
            if self.lexer.next() == Token::End {
                return;
            }
        }
    }

    /// IMPORT a.b AS c, EXPORT a AS b.
    fn skip_statement(&mut self) {
        self.lexer.next();
        self.lexer.next();
        if self.lexer.peek() == Token::Word("AS") {
            self.lexer.next();
            self.lexer.next();
        }
    }

    /// UNIT length millimetres 0.001 (the factor gives metres).
    fn unit_statement(&mut self) {
        self.lexer.next();
        let category = self.lexer.next();
        let _name = self.lexer.next();
        if let (Token::Word("length"), Token::Number(f)) = (category, self.lexer.peek()) {
            self.lexer.next();
            if f > 0.0 && f.is_finite() {
                self.unit = Some(f * 1000.0);
            }
        }
    }

    fn skip_proto(&mut self) -> Result<(), String> {
        let extern_ = self.lexer.next() == Token::Word("EXTERNPROTO");
        if let Token::Word(name) = self.lexer.next() {
            self.protos.push(name.to_string());
        }
        self.skip_group(Token::OpenList, Token::CloseList)?;
        if extern_ {
            // The URL: a string or a list of them.
            if self.lexer.peek() == Token::OpenList {
                self.skip_group(Token::OpenList, Token::CloseList)?;
            } else {
                self.lexer.next();
            }
            Ok(())
        } else {
            self.skip_group(Token::Open, Token::Close)
        }
    }

    fn skip_group(&mut self, open: Token, close: Token) -> Result<(), String> {
        if self.lexer.next() != open {
            return Err(self.bad("a PROTO is not written as expected"));
        }
        let mut depth = 1;
        while depth > 0 {
            match self.lexer.next() {
                Token::End => return Err(self.bad("a PROTO is never closed")),
                t if t == open => depth += 1,
                t if t == close => depth -= 1,
                _ => {}
            }
        }
        Ok(())
    }

    /// DEF name Node {…}, USE name, or Node {…}; None for NULL or a USE of an unknown name.
    fn node(&mut self, depth: usize) -> Result<Option<Rc<Node>>, String> {
        if depth > MAX_NESTING {
            return Err(self.bad("nodes are nested too deeply"));
        }
        let name;
        match self.lexer.next() {
            Token::Word("NULL") => return Ok(None),
            Token::Word("USE") => {
                return match self.lexer.next() {
                    Token::Word(n) => Ok(self.defs.get(n).cloned()),
                    _ => Err(self.bad("USE without a name")),
                };
            }
            Token::Word("DEF") => match self.lexer.next() {
                Token::Word(n) => name = Some(n.to_string()),
                _ => return Err(self.bad("DEF without a name")),
            },
            Token::Word(kind) => return self.body(kind, None, depth).map(Some),
            _ => return Err(self.bad("a node was expected")),
        }
        match self.lexer.next() {
            Token::Word(kind) => {
                let n = self.body(kind, name.clone(), depth)?;
                if let Some(name) = name {
                    self.defs.insert(name, n.clone());
                }
                Ok(Some(n))
            }
            _ => Err(self.bad("a node type was expected after DEF")),
        }
    }

    fn body(&mut self, kind: &str, name: Option<String>, depth: usize) -> Result<Rc<Node>, String> {
        if self.lexer.next() != Token::Open {
            return Err(self.bad(&format!("{kind} is not followed by {{")));
        }
        let mut node = Node::new(kind);
        node.name = name;
        loop {
            self.report();
            match self.lexer.next() {
                Token::Close => break,
                Token::End => return Err(self.bad(&format!("{kind} is never closed"))),
                Token::Word(w) => {
                    // Interface declarations of Script nodes: kind type name [value].
                    if matches!(
                        w,
                        "field" | "exposedField" | "initializeOnly" | "inputOutput"
                    ) {
                        self.lexer.next();
                        if let Token::Word(f) = self.lexer.next()
                            && let Some(v) = self.value(depth)?
                        {
                            node.fields.push((f.to_string(), v));
                        }
                        continue;
                    }
                    if matches!(w, "eventIn" | "eventOut" | "inputOnly" | "outputOnly") {
                        self.skip_words(2);
                        continue;
                    }
                    if self.lexer.peek() == Token::Word("IS") {
                        self.skip_words(2);
                        continue;
                    }
                    if let Some(v) = self.value(depth)? {
                        node.fields.push((w.to_string(), v));
                    }
                }
                _ => return Err(self.bad(&format!("a field name was expected in {kind}"))),
            }
        }
        Ok(Rc::new(node))
    }

    /// A field's value; None for NULL or nothing usable.
    fn value(&mut self, depth: usize) -> Result<Option<Field>, String> {
        match self.lexer.peek() {
            Token::Number(_) => {
                let mut v = Vec::new();
                while let Token::Number(x) = self.lexer.peek() {
                    self.lexer.next();
                    v.push(x);
                }
                Ok(Some(Field::Numbers(v)))
            }
            Token::Text(t) => {
                self.lexer.next();
                Ok(Some(Field::Text(vec![t.to_string()])))
            }
            Token::Word("TRUE") => {
                self.lexer.next();
                Ok(Some(Field::Bool(true)))
            }
            Token::Word("FALSE") => {
                self.lexer.next();
                Ok(Some(Field::Bool(false)))
            }
            Token::Word("NULL") => {
                self.lexer.next();
                Ok(None)
            }
            Token::Word(_) => Ok(self.node(depth + 1)?.map(|n| Field::Nodes(vec![n]))),
            Token::OpenList => {
                self.lexer.next();
                self.list(depth)
            }
            _ => Err(self.bad("a value was expected")),
        }
    }

    fn list(&mut self, depth: usize) -> Result<Option<Field>, String> {
        let mut numbers = Vec::new();
        let mut texts = Vec::new();
        let mut nodes = Vec::new();
        loop {
            match self.lexer.peek() {
                Token::CloseList => {
                    self.lexer.next();
                    break;
                }
                Token::End => return Err(self.bad("a [ is never closed")),
                Token::Number(x) => {
                    self.lexer.next();
                    numbers.push(x);
                    // Long number lists are most of a big file: read them without the loop's checks.
                    while let Token::Number(x) = self.lexer.peek() {
                        self.lexer.next();
                        numbers.push(x);
                    }
                    self.report();
                }
                Token::Text(t) => {
                    self.lexer.next();
                    texts.push(t.to_string());
                }
                Token::Word("TRUE" | "FALSE") => {
                    self.lexer.next();
                }
                Token::Word(_) => {
                    if let Some(n) = self.node(depth + 1)? {
                        nodes.push(n);
                    }
                }
                _ => return Err(self.bad("a list holds something it cannot")),
            }
        }
        Ok(Some(if !nodes.is_empty() {
            Field::Nodes(nodes)
        } else if !texts.is_empty() {
            Field::Text(texts)
        } else {
            Field::Numbers(numbers)
        }))
    }
}

// ---------------------------------------------------------------------------------------------
// Export

/// A name VRML and X3D accept for DEF: letters, digits and underscores, not starting with a digit.
pub(crate) fn def_name(name: &str, taken: &mut std::collections::HashSet<String>) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert_str(0, "Part_");
    }
    let base = out.clone();
    let mut n = 2;
    while !taken.insert(out.clone()) {
        out = format!("{base}_{n}");
        n += 1;
    }
    out
}

/// VRML 2.0, one Shape per part, millimetres.
pub fn write<W: Write>(mut w: W, parts: &[NamedMesh]) -> io::Result<W> {
    writeln!(w, "#VRML V2.0 utf8")?;
    writeln!(w, "# tricky3D, millimetres")?;
    let mut taken = std::collections::HashSet::new();
    for p in parts {
        let name = def_name(p.name, &mut taken);
        writeln!(w, "DEF {name} Shape {{")?;
        writeln!(w, "  appearance Appearance {{ material Material {{ }} }}")?;
        writeln!(w, "  geometry IndexedFaceSet {{")?;
        writeln!(w, "    solid TRUE")?;
        writeln!(w, "    coord Coordinate {{ point [")?;
        for v in p.mesh.positions.as_chunks::<3>().0 {
            writeln!(w, "      {} {} {},", v[0], v[1], v[2])?;
        }
        writeln!(w, "    ] }}")?;
        writeln!(w, "    coordIndex [")?;
        for t in p.mesh.indices.as_chunks::<3>().0 {
            writeln!(w, "      {}, {}, {}, -1,", t[0], t[1], t[2])?;
        }
        writeln!(w, "    ]")?;
        writeln!(w, "  }}")?;
        writeln!(w, "}}")?;
    }
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::stats;

    fn read_text(text: &str) -> Result<Loaded, String> {
        read(text.as_bytes(), &mut Progress::silent())
    }

    /// A 10 mm cube as an IndexedFaceSet of quads, inside a transform, with a material.
    pub(crate) const CUBE: &str = "#VRML V2.0 utf8
# a comment, with \"quotes\" and { braces }
WorldInfo { title \"cube\" info [ \"a\" \"b\" ] }
DEF Moved Transform {
  translation 5 0 0
  children [
    DEF Cube Shape {
      appearance Appearance { material Material { diffuseColor 1 0 0 } }
      geometry IndexedFaceSet {
        coord DEF Pts Coordinate { point [ 0 0 0, 10 0 0, 10 10 0, 0 10 0,
                                           0 0 10, 10 0 10, 10 10 10, 0 10 10 ] }
        coordIndex [ 0 3 2 1 -1, 4 5 6 7 -1, 0 1 5 4 -1, 1 2 6 5 -1, 2 3 7 6 -1, 3 0 4 7 -1 ]
      }
    }
  ]
}
ROUTE A.b TO C.d
";

    #[test]
    fn reads_a_cube_with_its_transform_and_colour() {
        let loaded = read_text(CUBE).unwrap();
        assert_eq!(loaded.objects.len(), 1);
        let o = &loaded.objects[0];
        assert_eq!(o.name.as_deref(), Some("Cube"));
        assert_eq!(o.mesh.triangle_count(), 12);
        assert_eq!(o.mesh.vertex_count(), 8);
        let s = stats(&o.mesh);
        assert!((s.volume - 1000.0).abs() < 1e-3, "{}", s.volume);
        let b = crate::mesh::bounds(&o.mesh).unwrap();
        assert_eq!(b, [[5.0, 0.0, 0.0], [15.0, 10.0, 10.0]]);
        assert_eq!(o.color, Some([255, 0, 0]));
        assert!(o.colors.is_none());
        assert_eq!(loaded.unit, None);
    }

    #[test]
    fn use_repeats_a_shape_and_per_face_colours_come_through() {
        let text = format!(
            "{CUBE}
Transform {{ translation 0 0 20 children USE Cube }}
Shape {{
  geometry IndexedFaceSet {{
    coord USE Pts
    coordIndex [ 0 3 2 1 -1, 4 5 6 7 -1, 0 1 5 4 -1, 1 2 6 5 -1, 2 3 7 6 -1, 3 0 4 7 -1 ]
    colorPerVertex FALSE
    color Color {{ color [ 1 0 0, 0 1 0 ] }}
    colorIndex [ 0 0 1 1 1 1 ]
  }}
}}"
        );
        let loaded = read_text(&text).unwrap();
        // Both uses of "Cube" are one object; the unnamed shape is another.
        assert_eq!(loaded.objects.len(), 2);
        assert_eq!(loaded.objects[0].mesh.triangle_count(), 24);
        let colors = loaded.objects[1].colors.as_ref().unwrap();
        assert_eq!(colors.len(), 12 * 3);
        assert_eq!(&colors[..6], &[255, 0, 0, 255, 0, 0]);
        assert_eq!(&colors[colors.len() - 3..], &[0, 255, 0]);
    }

    #[test]
    fn x3d_classic_reads_its_unit_and_vrml_1_is_refused() {
        let text = "#X3D V3.3 utf8\nPROFILE Interchange\nUNIT length centimetres 0.01\n\
                    Shape { geometry Box { size 1 2 3 } }";
        let loaded = read_text(text).unwrap();
        assert_eq!(loaded.unit, Some(10.0));
        let b = crate::mesh::bounds(&loaded.objects[0].mesh).unwrap();
        assert!((b[1][2] - b[0][2] - 30.0).abs() < 1e-4, "{b:?}");
        assert_eq!(
            read_text("#VRML V1.0 ascii\nSeparator { }").unwrap_err(),
            "it is a VRML 1.0 file, which tricky3D cannot open (only VRML 2.0 and VRML97)"
        );
        assert!(read_text("solid x").is_err());
        assert!(read_text("#VRML V2.0 utf8\nShape { geometry Box {").is_err());
    }

    #[test]
    fn a_proto_is_skipped_with_a_warning() {
        let text = "#VRML V2.0 utf8\nPROTO Wheel [ field SFFloat r 1 ] { Shape { geometry Sphere { radius IS r } } }\n\
                    Wheel { r 2 }\nShape { geometry Box { } }";
        let loaded = read_text(text).unwrap();
        assert_eq!(loaded.objects.len(), 1);
        assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
        assert!(loaded.warnings[0].contains("Wheel"));
    }

    #[test]
    fn writes_what_it_reads() {
        let loaded = read_text(CUBE).unwrap();
        let mesh = &loaded.objects[0].mesh;
        let parts = [NamedMesh {
            name: "a cube",
            mesh,
        }];
        let bytes = write(Vec::new(), &parts).unwrap();
        let back = read(&bytes, &mut Progress::silent()).unwrap();
        assert_eq!(back.objects[0].name.as_deref(), Some("a_cube"));
        assert_eq!(back.objects[0].mesh, *mesh);
    }

    #[test]
    fn a_gzip_file_unpacks() {
        let bytes = crate::formats::xml::tests_gzip(CUBE.as_bytes());
        let loaded = read(&bytes, &mut Progress::silent()).unwrap();
        assert_eq!(loaded.objects[0].mesh.triangle_count(), 12);
    }
}
