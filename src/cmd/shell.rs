//! `shell` subcommand: an interactive SQLite REPL backed by the decrypted
//! scratch copy. Uses rustyline for history, line editing, and Ctrl-R search.

use crate::db::open::{open_encrypted, OpenMode};
use crate::fmt::output::{render, Format, ResultSet};

use anyhow::{anyhow, Context, Result};
use clap::Args;
use rusqlite::Connection;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Args, Debug)]
pub struct ShellArgs {
    /// Path to the encrypted .db file.
    pub input: PathBuf,

    /// User UID (numeric). Defaults to the value parsed from the filename.
    #[arg(short, long)]
    pub uid: Option<String>,

    /// Allow writes (UPDATE / INSERT / DELETE / DDL). Writes only affect the
    /// scratch copy and are discarded when the shell exits.
    #[arg(long)]
    pub write: bool,

    /// Initial output format. Toggleable from the prompt with `.mode`.
    #[arg(long, value_parser = ["table", "csv", "json"], default_value = "table")]
    pub mode: String,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn history_path() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".aweme-db-decrypt-history"))
}

fn parse_format(s: &str) -> Option<Format> {
    match s {
        "table" => Some(Format::Table),
        "csv" => Some(Format::Csv),
        "json" => Some(Format::Json),
        _ => None,
    }
}

fn print_help() {
    println!(
        "Meta commands:\n\
         \x20 .help                 show this help\n\
         \x20 .exit | .quit         exit the shell\n\
         \x20 .tables               list user tables\n\
         \x20 .schema [TABLE]       show CREATE statements\n\
         \x20 .mode table|csv|json  switch output format\n\
         \x20 .read PATH            execute SQL from file\n\n\
         Anything else is SQL. Statements are submitted on a trailing semicolon;\n\
         multi-line input is accumulated until then. Ctrl-C clears the buffer,\n\
         Ctrl-D exits."
    );
}

fn list_tables(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master \
         WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' \
         ORDER BY name",
    )?;
    let names: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    if names.is_empty() {
        println!("(no tables)");
    } else {
        for n in names {
            println!("{n}");
        }
    }
    Ok(())
}

fn show_schema(conn: &Connection, table: Option<&str>) -> Result<()> {
    let emit = |sql_text: Option<String>, any: &mut bool| {
        if let Some(s) = sql_text {
            *any = true;
            println!("{s};");
        }
    };
    let mut any = false;
    match table {
        Some(t) => {
            let mut stmt = conn.prepare(
                "SELECT sql FROM sqlite_master \
                 WHERE type IN ('table','view','index','trigger') AND name = ?1 \
                 ORDER BY type, name",
            )?;
            let mut rows = stmt.query([t])?;
            while let Some(r) = rows.next()? {
                emit(r.get(0)?, &mut any);
            }
        }
        None => {
            let mut stmt = conn.prepare(
                "SELECT sql FROM sqlite_master \
                 WHERE type IN ('table','view','index','trigger') AND sql IS NOT NULL \
                 ORDER BY type, name",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                emit(r.get(0)?, &mut any);
            }
        }
    }
    if !any {
        println!("(no matching schema objects)");
    }
    Ok(())
}

fn run_read(conn: &Connection, path: &Path, fmt: Format) -> Result<()> {
    let body = std::fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    for stmt in crate::fmt::sql_split::split_statements(&body) {
        if let Err(e) = exec_sql(conn, &stmt, fmt) {
            eprintln!("error: {e:#}");
        }
    }
    Ok(())
}

fn exec_sql(conn: &Connection, sql: &str, fmt: Format) -> Result<()> {
    let mut stmt = conn.prepare(sql).map_err(|e| anyhow!(e))?;
    if stmt.column_count() == 0 {
        let n = stmt.execute([])?;
        drop(stmt);
        println!("[+] ok ({n} row{} changed)", if n == 1 { "" } else { "s" });
        return Ok(());
    }
    let rs = ResultSet::collect(&mut stmt)?;
    drop(stmt);
    let stdout = io::stdout();
    let mut w = stdout.lock();
    render(&rs, fmt, &mut w)?;
    w.flush().ok();
    Ok(())
}

/// Returns true if the line should be treated as a meta-command (handled
/// immediately rather than accumulated into the SQL buffer).
fn is_meta(line: &str) -> bool {
    line.trim_start().starts_with('.')
}

fn handle_meta(conn: &Connection, line: &str, fmt: &mut Format) -> Result<bool> {
    let line = line.trim();
    let mut parts = line.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    match cmd {
        ".exit" | ".quit" => return Ok(true),
        ".help" => print_help(),
        ".tables" => list_tables(conn)?,
        ".schema" => {
            let table = if rest.is_empty() { None } else { Some(rest) };
            show_schema(conn, table)?;
        }
        ".mode" => match parse_format(rest) {
            Some(f) => {
                *fmt = f;
                println!("[+] mode = {rest}");
            }
            None => eprintln!("usage: .mode table|csv|json"),
        },
        ".read" => {
            if rest.is_empty() {
                eprintln!("usage: .read <path>");
            } else {
                run_read(conn, Path::new(rest), *fmt)?;
            }
        }
        other => eprintln!("unknown meta command: {other} (try .help)"),
    }
    Ok(false)
}

pub fn run(args: ShellArgs) -> Result<()> {
    let mode = if args.write { OpenMode::ReadWrite } else { OpenMode::ReadOnly };
    let opened = open_encrypted(&args.input, args.uid.as_deref(), mode)?;
    let mut fmt = parse_format(&args.mode).unwrap_or(Format::Table);

    println!(
        "aweme-db-decrypt shell — {} (uid {}); {}",
        opened.kind.label(),
        opened.uid,
        if matches!(mode, OpenMode::ReadOnly) {
            "read-only scratch copy"
        } else {
            "writable scratch copy (changes discarded on exit)"
        }
    );
    println!("type .help for meta commands, Ctrl-D to exit\n");

    let mut rl = DefaultEditor::new()?;
    let history = history_path();
    if let Some(p) = &history {
        let _ = rl.load_history(p);
    }

    let mut buf = String::new();
    loop {
        let prompt = if buf.is_empty() { "sqlite> " } else { "   ...> " };
        match rl.readline(prompt) {
            Ok(line) => {
                if buf.is_empty() && is_meta(&line) {
                    let _ = rl.add_history_entry(line.as_str());
                    match handle_meta(&opened.conn, &line, &mut fmt) {
                        Ok(true) => break,
                        Ok(false) => {}
                        Err(e) => eprintln!("error: {e:#}"),
                    }
                    continue;
                }
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(&line);
                // Submit on a trailing `;` (after stripping line comments).
                if buf.trim_end().ends_with(';') {
                    let _ = rl.add_history_entry(buf.as_str());
                    for stmt in crate::fmt::sql_split::split_statements(&buf) {
                        if let Err(e) = exec_sql(&opened.conn, &stmt, fmt) {
                            eprintln!("error: {e:#}");
                        }
                    }
                    buf.clear();
                }
            }
            Err(ReadlineError::Interrupted) => {
                if buf.is_empty() {
                    println!("(use .exit or Ctrl-D to quit)");
                } else {
                    buf.clear();
                }
            }
            Err(ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("readline error: {e}");
                break;
            }
        }
    }

    if let Some(p) = &history {
        let _ = rl.save_history(p);
    }
    Ok(())
}
