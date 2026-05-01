//! `decrypt` subcommand: produce a plaintext SQLite copy of the encrypted DB.

use crate::cipher::{
    apply_v3_pragmas, passphrase_for_uid, resolve_kind_uid, verify_open,
};
use crate::workdir::{nice_path, with_suffix, WorkDir};

use anyhow::{anyhow, bail, Context, Result};
use clap::Args;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

#[derive(Args, Debug)]
pub struct DecryptArgs {
    /// Path to the encrypted .db file.
    pub input: PathBuf,

    /// Output plaintext SQLite file. Default: derived from the input filename.
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// User UID (numeric). Defaults to the value parsed from the filename.
    #[arg(short, long)]
    pub uid: Option<String>,

    /// Suppress the post-decryption table summary.
    #[arg(short = 'q', long)]
    pub quiet: bool,

    /// Print sensitive details (full passphrase). Off by default.
    #[arg(short, long)]
    pub verbose: bool,

    /// Overwrite the output file if it already exists.
    #[arg(short, long)]
    pub force: bool,
}

fn default_output(input: &Path) -> PathBuf {
    let stem = input
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("decrypted.db");
    let plain = stem.strip_prefix("encrypted_").unwrap_or(stem);
    let plain = format!("plain_{plain}");
    input
        .parent()
        .map(|p| p.join(&plain))
        .unwrap_or_else(|| PathBuf::from(plain))
}

fn print_summary(conn: &Connection) -> Result<()> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
    let names: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    println!("[+] tables ({}):", names.len());
    for n in &names {
        println!("      {n}");
    }
    let interesting = [
        "msg",
        "conversation_core",
        "conversation_list",
        "participant",
        "participant_read",
        "attchment",
        "share_merge_msg",
        "SIMPLE_USER",
        "GROUP_LIST",
        "STRANGER_LIST",
    ];
    let present: Vec<&str> = interesting
        .iter()
        .copied()
        .filter(|t| names.iter().any(|n| n == t))
        .collect();
    if !present.is_empty() {
        println!("[+] row counts:");
        for t in present {
            // Table names come from a hard-coded whitelist, so the format!() is
            // not a SQL-injection vector.
            match conn.query_row::<i64, _, _>(
                &format!("SELECT count(*) FROM \"{t}\""),
                [],
                |r| r.get(0),
            ) {
                Ok(n) => println!("      {t:<24} {n}"),
                Err(e) => eprintln!("      {t:<24} (query failed: {e})"),
            }
        }
    }
    Ok(())
}

fn attach_and_export(conn: &Connection, target: &Path) -> Result<()> {
    let s = target
        .to_str()
        .ok_or_else(|| anyhow!("output path not utf-8"))?;
    if s.bytes()
        .any(|b| b == b'\'' || b == b'\0' || b == b'\n' || b == b'\r')
    {
        bail!("output path contains forbidden characters: {:?}", s);
    }
    conn.execute(&format!("ATTACH DATABASE '{}' AS plain KEY ''", s), [])
        .context("ATTACH plaintext target failed")?;
    // sqlcipher_export() returns one NULL row; query_row + discard consumes it.
    // (execute() rejects statements that produce rows, so we cannot use it.)
    conn.query_row("SELECT sqlcipher_export('plain')", [], |_| Ok(()))
        .context("sqlcipher_export failed")?;
    conn.execute("DETACH DATABASE plain", [])?;
    Ok(())
}

pub fn run(args: DecryptArgs) -> Result<()> {
    let input = nice_path(
        args.input
            .canonicalize()
            .with_context(|| format!("input file not found: {}", args.input.display()))?,
    );
    let filename = input
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("invalid filename"))?;

    let (kind, uid) = resolve_kind_uid(filename, args.uid.as_deref())?;
    let password = passphrase_for_uid(&uid);

    let output = args.output.unwrap_or_else(|| default_output(&input));
    if output.exists() && !args.force {
        bail!(
            "output {} already exists — pass --force to overwrite",
            output.display()
        );
    }

    println!("[+] input    : {}", input.display());
    println!(
        "[+] kind     : {} (schema baseline v{})",
        kind.label(),
        kind.schema_baseline()
    );
    println!("[+] uid      : {}", uid);
    if args.verbose {
        println!("[+] password : {}", password);
    } else {
        println!("[+] password : (suppressed; pass --verbose to display)");
    }
    println!("[+] cipher   : SQLCipher v3 (AES-256-CBC, HMAC-SHA1, PBKDF2-SHA1 64000, 4096 page)");
    println!("[+] output   : {}", output.display());

    if with_suffix(&input, "-wal").exists() {
        println!("[i] -wal     : found, will be replayed during decryption");
    }
    println!();

    let output_dir = output
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("create output dir {}", output_dir.display()))?;

    // Decryption runs entirely inside the workdir; the source files on disk are
    // untouched. The staged plaintext is atomic-renamed into place only after
    // sqlcipher_export succeeds.
    let work = WorkDir::create(&input, &output_dir).context("preparing work directory")?;

    let conn = Connection::open_with_flags(
        &work.db_copy,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open failed: {}", work.db_copy.display()))?;

    apply_v3_pragmas(&conn, &password)?;
    let object_count = verify_open(&conn)?;
    println!("[+] decrypted: {} schema objects in sqlite_master", object_count);

    if let Ok(uv) = conn.query_row::<i64, _, _>("PRAGMA user_version", [], |r| r.get(0)) {
        if uv != 0 && uv != kind.schema_baseline() as i64 {
            println!(
                "[i] schema   : user_version={} (baseline for this kind = {}, normal after SDK upgrade)",
                uv,
                kind.schema_baseline()
            );
        }
    }

    attach_and_export(&conn, &work.staged_plain)?;
    drop(conn);

    if output.exists() {
        std::fs::remove_file(&output)
            .with_context(|| format!("remove existing output {}", output.display()))?;
    }
    if let Err(e) = std::fs::rename(&work.staged_plain, &output) {
        // workdir is colocated with output so this should not happen in practice;
        // be defensive and fall back to copy.
        eprintln!("[i] rename failed ({e}); falling back to copy");
        std::fs::copy(&work.staged_plain, &output)?;
    }

    let bytes = std::fs::metadata(&output)?.len();
    println!("[+] wrote    : {} ({} bytes)", output.display(), bytes);

    if !args.quiet {
        let plain = Connection::open_with_flags(
            &output,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        println!();
        print_summary(&plain)?;
    }

    Ok(())
}
