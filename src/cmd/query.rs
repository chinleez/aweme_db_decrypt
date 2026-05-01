//! `query` subcommand: run one or more SQL statements against an encrypted DB
//! and print the result set(s) as a table (default), CSV, or JSON.

use crate::db::open::{open_encrypted, OpenMode};
use crate::fmt::output::{render, Format, ResultSet};
use crate::fmt::sql_split::split_statements;

use anyhow::{anyhow, bail, Context, Result};
use clap::Args;
use std::io::{self, Write};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct QueryArgs {
    /// Path to the encrypted .db file.
    pub input: PathBuf,

    /// User UID (numeric). Defaults to the value parsed from the filename.
    #[arg(short, long)]
    pub uid: Option<String>,

    /// SQL to execute. Can be passed multiple times; statements run in order.
    #[arg(short = 'e', long = "execute", value_name = "SQL")]
    pub execute: Vec<String>,

    /// Read SQL from a file. Mutually exclusive with -e.
    #[arg(short = 'f', long = "file", value_name = "PATH", conflicts_with = "execute")]
    pub file: Option<PathBuf>,

    /// Emit JSON arrays instead of an aligned table.
    #[arg(long, conflicts_with = "csv")]
    pub json: bool,

    /// Emit RFC 4180 CSV instead of an aligned table.
    #[arg(long)]
    pub csv: bool,

    /// Allow writes (UPDATE / INSERT / DELETE / DDL). Writes only affect the
    /// scratch copy and are discarded when the process exits.
    #[arg(long)]
    pub write: bool,
}

fn pick_format(args: &QueryArgs) -> Format {
    if args.json {
        Format::Json
    } else if args.csv {
        Format::Csv
    } else {
        Format::Table
    }
}

pub fn run(args: QueryArgs) -> Result<()> {
    let mut sql_chunks: Vec<String> = Vec::new();
    if let Some(path) = &args.file {
        let body = std::fs::read_to_string(path)
            .with_context(|| format!("read SQL file {}", path.display()))?;
        sql_chunks.push(body);
    }
    sql_chunks.extend(args.execute.iter().cloned());
    if sql_chunks.is_empty() {
        bail!("no SQL provided — use -e \"...\" (repeatable) or -f <file>");
    }

    let mode = if args.write { OpenMode::ReadWrite } else { OpenMode::ReadOnly };
    let opened = open_encrypted(&args.input, args.uid.as_deref(), mode)?;
    let fmt = pick_format(&args);

    let stdout = io::stdout();
    let mut out = stdout.lock();

    let mut first_set = true;
    for chunk in &sql_chunks {
        for stmt_sql in split_statements(chunk) {
            run_one(&opened.conn, &stmt_sql, fmt, &mut out, &mut first_set)
                .with_context(|| format!("statement failed: {stmt_sql}"))?;
        }
    }

    out.flush().ok();
    Ok(())
}

fn run_one(
    conn: &rusqlite::Connection,
    sql: &str,
    fmt: Format,
    w: &mut impl Write,
    first_set: &mut bool,
) -> Result<()> {
    let mut stmt = conn.prepare(sql).map_err(|e| anyhow!(e))?;
    if stmt.column_count() == 0 {
        // No result set (DDL / INSERT / UPDATE / DELETE). Drop stmt first so
        // the borrow on conn is released, then read changes() afterward.
        let n = stmt.execute([])?;
        drop(stmt);
        // Print a small acknowledgment to stderr so it doesn't pollute piped
        // table/csv/json output on stdout.
        eprintln!("[+] ok ({n} row{} changed)", if n == 1 { "" } else { "s" });
        return Ok(());
    }

    let rs = ResultSet::collect(&mut stmt)?;
    drop(stmt);

    // Separate multiple result sets. For JSON we emit one document per set,
    // separated by blank lines (caller can split on `^\[$`); for table/csv
    // a blank line is enough to keep them visually distinct.
    if !*first_set {
        writeln!(w)?;
    }
    *first_set = false;
    render(&rs, fmt, w)?;
    Ok(())
}
