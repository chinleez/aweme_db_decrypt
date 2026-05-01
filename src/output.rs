//! Rendering of SQLite result sets in table, CSV and JSON form.

use anyhow::Result;
use rusqlite::types::Value;
use rusqlite::Statement;
use std::io::{self, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Table,
    Csv,
    Json,
}

/// One materialized result set. Column types are preserved per cell so the
/// renderer can format JSON numerically and table mode can show NULL distinctly.
pub struct ResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

impl ResultSet {
    pub fn collect(stmt: &mut Statement<'_>) -> Result<ResultSet> {
        let columns: Vec<String> =
            stmt.column_names().into_iter().map(|s| s.to_string()).collect();
        let n = columns.len();
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            let mut row = Vec::with_capacity(n);
            for i in 0..n {
                let v: Value = r.get_ref(i)?.into();
                row.push(v);
            }
            out.push(row);
        }
        Ok(ResultSet { columns, rows: out })
    }
}

pub fn render(rs: &ResultSet, fmt: Format, w: &mut impl Write) -> io::Result<()> {
    match fmt {
        Format::Table => render_table(rs, w),
        Format::Csv => render_csv(rs, w),
        Format::Json => render_json(rs, w),
    }
}

fn cell_display(v: &Value) -> String {
    match v {
        Value::Null => "NULL".to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Real(f) => format!("{f}"),
        Value::Text(s) => s.clone(),
        Value::Blob(b) => format!("<blob:{} bytes>", b.len()),
    }
}

fn display_width(s: &str) -> usize {
    // Use char count rather than byte length so multi-byte UTF-8 (CJK)
    // doesn't blow up the column width. This is still imperfect — wide
    // glyphs (most CJK) actually take 2 terminal cells — but the simple
    // count keeps borders aligned for ASCII-heavy data and stays close
    // for everything else without pulling in a Unicode-width crate.
    s.chars().count()
}

fn render_table(rs: &ResultSet, w: &mut impl Write) -> io::Result<()> {
    let n = rs.columns.len();
    if n == 0 {
        writeln!(w, "(no columns)")?;
        return Ok(());
    }

    let mut widths: Vec<usize> = rs.columns.iter().map(|s| display_width(s)).collect();
    let cells: Vec<Vec<String>> = rs
        .rows
        .iter()
        .map(|row| row.iter().map(cell_display).collect())
        .collect();
    for row in &cells {
        for (i, c) in row.iter().enumerate() {
            let cw = display_width(c);
            if cw > widths[i] {
                widths[i] = cw;
            }
        }
    }

    // header
    let mut first = true;
    for (i, name) in rs.columns.iter().enumerate() {
        if !first {
            write!(w, "  ")?;
        }
        first = false;
        let pad = widths[i].saturating_sub(display_width(name));
        write!(w, "{name}{:pad$}", "", pad = pad)?;
    }
    writeln!(w)?;

    // separator
    let mut first = true;
    for (i, _) in rs.columns.iter().enumerate() {
        if !first {
            write!(w, "  ")?;
        }
        first = false;
        for _ in 0..widths[i] {
            write!(w, "-")?;
        }
    }
    writeln!(w)?;

    // rows
    for row in &cells {
        let mut first = true;
        for (i, c) in row.iter().enumerate() {
            if !first {
                write!(w, "  ")?;
            }
            first = false;
            let pad = widths[i].saturating_sub(display_width(c));
            write!(w, "{c}{:pad$}", "", pad = pad)?;
        }
        writeln!(w)?;
    }

    writeln!(w, "({} row{})", rs.rows.len(), if rs.rows.len() == 1 { "" } else { "s" })?;
    Ok(())
}

fn render_csv(rs: &ResultSet, w: &mut impl Write) -> io::Result<()> {
    write_csv_row(w, rs.columns.iter().map(|s| s.as_str()))?;
    for row in &rs.rows {
        let cells: Vec<String> = row
            .iter()
            .map(|v| match v {
                Value::Null => String::new(),
                Value::Integer(i) => i.to_string(),
                Value::Real(f) => format!("{f}"),
                Value::Text(s) => s.clone(),
                Value::Blob(b) => hex_encode(b),
            })
            .collect();
        write_csv_row(w, cells.iter().map(|s| s.as_str()))?;
    }
    Ok(())
}

fn write_csv_row<'a>(
    w: &mut impl Write,
    cells: impl IntoIterator<Item = &'a str>,
) -> io::Result<()> {
    let mut first = true;
    for c in cells {
        if !first {
            write!(w, ",")?;
        }
        first = false;
        if c.contains([',', '"', '\n', '\r']) {
            write!(w, "\"")?;
            for ch in c.chars() {
                if ch == '"' {
                    write!(w, "\"\"")?;
                } else {
                    write!(w, "{ch}")?;
                }
            }
            write!(w, "\"")?;
        } else {
            write!(w, "{c}")?;
        }
    }
    writeln!(w)?;
    Ok(())
}

fn render_json(rs: &ResultSet, w: &mut impl Write) -> io::Result<()> {
    writeln!(w, "[")?;
    let last = rs.rows.len();
    for (ri, row) in rs.rows.iter().enumerate() {
        write!(w, "  {{")?;
        for (ci, (name, v)) in rs.columns.iter().zip(row).enumerate() {
            if ci > 0 {
                write!(w, ", ")?;
            }
            write!(w, "\"")?;
            write_json_str(w, name)?;
            write!(w, "\": ")?;
            write_json_value(w, v)?;
        }
        if ri + 1 < last {
            writeln!(w, "}},")?;
        } else {
            writeln!(w, "}}")?;
        }
    }
    writeln!(w, "]")?;
    Ok(())
}

fn write_json_value(w: &mut impl Write, v: &Value) -> io::Result<()> {
    match v {
        Value::Null => write!(w, "null"),
        Value::Integer(i) => write!(w, "{i}"),
        Value::Real(f) => {
            if f.is_finite() {
                write!(w, "{f}")
            } else {
                // SQLite stores neither NaN nor +/-Inf, but be defensive
                write!(w, "null")
            }
        }
        Value::Text(s) => {
            write!(w, "\"")?;
            write_json_str(w, s)?;
            write!(w, "\"")
        }
        Value::Blob(b) => {
            // Encode blobs as hex strings, prefixed so a consumer can tell
            // them apart from regular text columns.
            write!(w, "\"hex:{}\"", hex_encode(b))
        }
    }
}

fn write_json_str(w: &mut impl Write, s: &str) -> io::Result<()> {
    for ch in s.chars() {
        match ch {
            '"' => write!(w, "\\\"")?,
            '\\' => write!(w, "\\\\")?,
            '\n' => write!(w, "\\n")?,
            '\r' => write!(w, "\\r")?,
            '\t' => write!(w, "\\t")?,
            '\x08' => write!(w, "\\b")?,
            '\x0c' => write!(w, "\\f")?,
            c if (c as u32) < 0x20 => write!(w, "\\u{:04x}", c as u32)?,
            c => write!(w, "{c}")?,
        }
    }
    Ok(())
}

fn hex_encode(b: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for &byte in b {
        s.push(HEX[(byte >> 4) as usize] as char);
        s.push(HEX[(byte & 0x0f) as usize] as char);
    }
    s
}

