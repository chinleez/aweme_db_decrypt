use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

/// Strip the Windows extended-length `\\?\` prefix from a canonicalized path
/// so display strings stay readable and SQLite ATTACH paths don't carry the
/// prefix. UNC paths (`\\?\UNC\...`) are left intact. No-op on non-Windows.
fn nice_path(p: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(s) = p.as_os_str().to_str() {
            if let Some(rest) = s.strip_prefix(r"\\?\") {
                if !rest.starts_with(r"UNC\") {
                    return PathBuf::from(rest);
                }
            }
        }
    }
    p
}

#[derive(Parser, Debug)]
#[command(
    name = "aweme-db-decrypt",
    about = "Decrypt encrypted IM databases shipped by com.ss.android.ugc.aweme.lite (抖音极速版)",
    long_about = "Decrypt encrypted IM SQLite databases of com.ss.android.ugc.aweme.lite \
                  into plain SQLite files. Supported filenames:\n  \
                  - encrypted_<uid>_im.db        (IM Core)\n  \
                  - encrypted_sub_<uid>_im.db    (IM Core, subprocess)\n  \
                  - encrypted_im_biz_<uid>.db    (IM Biz)\n\n\
                  The originals are never modified; decryption runs on a private copy."
)]
struct Cli {
    /// Path to the encrypted .db file.
    input: PathBuf,

    /// Output plaintext SQLite file. Default: derived from the input filename.
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// User UID (numeric). Defaults to the value parsed from the filename.
    #[arg(short, long)]
    uid: Option<String>,

    /// Suppress the post-decryption table summary.
    #[arg(short = 'q', long)]
    quiet: bool,

    /// Print sensitive details (full passphrase). Off by default.
    #[arg(short, long)]
    verbose: bool,

    /// Overwrite the output file if it already exists.
    #[arg(short, long)]
    force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DbKind {
    ImCore,
    ImBiz,
}

impl DbKind {
    fn schema_baseline(&self) -> i32 {
        match self {
            DbKind::ImCore => 73,
            DbKind::ImBiz => 56,
        }
    }
    fn label(&self) -> &'static str {
        match self {
            DbKind::ImCore => "IM Core (encrypted_<uid>_im.db)",
            DbKind::ImBiz => "IM Biz  (encrypted_im_biz_<uid>.db)",
        }
    }
}

fn validate_uid(uid: &str) -> Result<()> {
    if uid.is_empty() {
        bail!("uid is empty");
    }
    if !uid.chars().all(|c| c.is_ascii_digit()) {
        bail!("uid must contain only digits, got {:?}", uid);
    }
    Ok(())
}

fn detect(filename: &str) -> Result<(DbKind, String)> {
    if let Some(rest) = filename.strip_prefix("encrypted_im_biz_") {
        if let Some(uid) = rest.strip_suffix(".db") {
            if !uid.is_empty() && uid.chars().all(|c| c.is_ascii_digit()) {
                return Ok((DbKind::ImBiz, uid.to_string()));
            }
        }
    }
    if let Some(rest) = filename.strip_prefix("encrypted_") {
        let rest = rest.strip_prefix("sub_").unwrap_or(rest);
        if let Some(uid) = rest.strip_suffix("_im.db") {
            if !uid.is_empty() && uid.chars().all(|c| c.is_ascii_digit()) {
                return Ok((DbKind::ImCore, uid.to_string()));
            }
        }
    }
    Err(anyhow!(
        "cannot infer DB kind / uid from filename {:?} — pass --uid explicitly",
        filename
    ))
}

mod passphrase {
    pub const PREFIX: &str = "byte";
    pub const MIDDLE: &str = "imwcdb";
    pub const SUFFIX: &str = "dance";

    pub fn for_uid(uid: &str) -> String {
        let mut s = String::with_capacity(
            PREFIX.len() + MIDDLE.len() + SUFFIX.len() + uid.len() * 2,
        );
        s.push_str(PREFIX);
        s.push_str(uid);
        s.push_str(MIDDLE);
        s.push_str(uid);
        s.push_str(SUFFIX);
        s
    }
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

fn apply_v3_pragmas(conn: &Connection, password: &str) -> Result<()> {
    conn.pragma_update(None, "key", password)
        .context("PRAGMA key")?;
    conn.pragma_update(None, "cipher_page_size", 4096i64)?;
    conn.pragma_update(None, "kdf_iter", 64000i64)?;
    conn.pragma_update(None, "cipher_use_hmac", 1i64)?;
    conn.pragma_update(None, "cipher_hmac_algorithm", "HMAC_SHA1")?;
    conn.pragma_update(None, "cipher_kdf_algorithm", "PBKDF2_HMAC_SHA1")?;
    Ok(())
}

fn verify_open(conn: &Connection) -> Result<i64> {
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
        .map_err(|e| anyhow!("decryption failed (wrong uid / wrong cipher params): {e}"))
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

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Private workdir holding (a) a copy of the encrypted DB and (b) the staged
/// plaintext output. Removed on Drop. Lives next to the *output* directory so
/// the final atomic rename of the staged plaintext stays on the same FS.
struct WorkDir {
    root: PathBuf,
    db_copy: PathBuf,
    staged_plain: PathBuf,
}

impl WorkDir {
    fn create(input: &Path, output_dir: &Path) -> Result<Self> {
        let pid = std::process::id();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = output_dir.join(format!(".aweme-db-decrypt-{pid}-{now}"));
        std::fs::create_dir_all(&root)
            .with_context(|| format!("create workdir {}", root.display()))?;

        let stem = input
            .file_name()
            .ok_or_else(|| anyhow!("invalid input filename"))?;
        let db_copy = root.join(stem);
        std::fs::copy(input, &db_copy)
            .with_context(|| format!("copy input -> {}", db_copy.display()))?;
        for ext in ["-wal", "-shm", "-journal"] {
            let src = with_suffix(input, ext);
            if src.exists() {
                let dst = with_suffix(&db_copy, ext);
                std::fs::copy(&src, &dst)
                    .with_context(|| format!("copy sidecar {}", src.display()))?;
            }
        }

        let staged_plain = root.join("plain.staged.db");
        Ok(WorkDir { root, db_copy, staged_plain })
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
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

fn run(cli: Cli) -> Result<()> {
    let input = nice_path(
        cli.input
            .canonicalize()
            .with_context(|| format!("input file not found: {}", cli.input.display()))?,
    );
    let filename = input
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("invalid filename"))?;

    let (kind, uid) = match cli.uid.as_deref() {
        Some(u) => {
            validate_uid(u)?;
            match detect(filename) {
                Ok((k, detected)) => {
                    if detected != u {
                        eprintln!(
                            "[!] note: --uid {u} differs from filename uid {detected}; using --uid"
                        );
                    }
                    (k, u.to_string())
                }
                Err(_) => {
                    eprintln!("[i] note: filename does not match expected pattern; assuming IM Core");
                    (DbKind::ImCore, u.to_string())
                }
            }
        }
        None => detect(filename)?,
    };
    let password = passphrase::for_uid(&uid);

    let output = cli.output.unwrap_or_else(|| default_output(&input));
    if output.exists() && !cli.force {
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
    if cli.verbose {
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

    if !cli.quiet {
        let plain = Connection::open_with_flags(
            &output,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        println!();
        print_summary(&plain)?;
    }

    Ok(())
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
