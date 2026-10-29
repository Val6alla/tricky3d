//! List labels' CSV files: the rows of cells, with the separator guessed from the first lines
//! (comma, semicolon or tab) and cells in double quotes allowed to hold separators, quotes ("")
//! and line breaks. Spreadsheets on many systems write semicolons, so that guess matters.

/// Rows at most; a list makes at most `MAX_COPIES` copies anyway.
pub const MAX_ROWS: usize = 10_000;

/// The rows of `text`, each a list of cells, with spaces round cells trimmed and empty rows left
/// out. A byte order mark at the start is dropped.
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let separator = guess_separator(text);
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    cell.push('"');
                    chars.next();
                }
                '"' => quoted = false,
                _ => cell.push(c),
            }
            continue;
        }
        match c {
            '"' if cell.trim().is_empty() => {
                cell.clear();
                quoted = true;
            }
            c if c == separator => row.push(std::mem::take(&mut cell).trim().to_string()),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut cell).trim().to_string());
                finish(&mut rows, std::mem::take(&mut row));
                if rows.len() >= MAX_ROWS {
                    return rows;
                }
            }
            _ => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell.trim().to_string());
        finish(&mut rows, row);
    }
    rows
}

fn finish(rows: &mut Vec<Vec<String>>, row: Vec<String>) {
    if row.iter().any(|c| !c.is_empty()) {
        rows.push(row);
    }
}

/// The separator that splits the first lines most evenly, outside quotes.
fn guess_separator(text: &str) -> char {
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(10)
        .collect();
    let mut best = (',', 0usize);
    for sep in [',', ';', '\t'] {
        let counts: Vec<usize> = lines
            .iter()
            .map(|l| {
                let mut quoted = false;
                l.chars()
                    .filter(|&c| {
                        if c == '"' {
                            quoted = !quoted;
                        }
                        c == sep && !quoted
                    })
                    .count()
            })
            .collect();
        let least = counts.iter().copied().min().unwrap_or(0);
        if least > best.1 {
            best = (sep, least);
        }
    }
    best.0
}
