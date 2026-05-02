//! Shared "open an encrypted aweme.lite DB and hand back a live connection"
//! entry point used by the `query` and `shell` subcommands.

use super::cipher::{apply_v3_pragmas, passphrase_for_uid, resolve_kind_uid, verify_open, DbKind};
use super::workdir::{nice_path, WorkDir};

use anyhow::{anyhow, Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    ReadOnly,
    ReadWrite,
}

/// A live SQLCipher-decrypted connection backed by a private workdir copy.
/// Drop order matters: `conn` must be dropped before `_work`, which clears
/// the workdir from disk.
pub struct OpenedDb {
    pub conn: Connection,
    pub kind: DbKind,
    pub uid: String,
    // Kept for its Drop: removes the scratch workdir holding the DB copy.
    _work: WorkDir,
}

/// A live SQLCipher-decrypted connection opened directly on the source file.
/// This is intended for Android-side tailing where the source DB keeps changing
/// and a private copy would immediately become stale.
pub struct DirectOpenedDb {
    pub conn: Connection,
    pub kind: DbKind,
    pub uid: String,
}

pub fn open_encrypted(
    input: &Path,
    uid_override: Option<&str>,
    mode: OpenMode,
) -> Result<OpenedDb> {
    let input = nice_path(
        input
            .canonicalize()
            .with_context(|| format!("input file not found: {}", input.display()))?,
    );
    let filename = input
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("invalid filename"))?;

    let (kind, uid) = resolve_kind_uid(filename, uid_override)?;
    let password = passphrase_for_uid(&uid);

    // For query/shell we never want to touch the source directory; put the
    // scratch workdir under the OS temp dir so a read-only source tree still
    // works.
    let scratch_parent = std::env::temp_dir();
    std::fs::create_dir_all(&scratch_parent)
        .with_context(|| format!("create scratch parent {}", scratch_parent.display()))?;
    let work = WorkDir::create(&input, &scratch_parent).context("preparing work directory")?;

    let flags = match mode {
        OpenMode::ReadOnly => OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        OpenMode::ReadWrite => {
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
        }
    };
    let conn = Connection::open_with_flags(&work.db_copy, flags)
        .with_context(|| format!("open failed: {}", work.db_copy.display()))?;

    apply_v3_pragmas(&conn, &password)?;
    verify_open(&conn)?;

    Ok(OpenedDb { conn, kind, uid, _work: work })
}

pub fn open_encrypted_direct(input: &Path, uid_override: Option<&str>) -> Result<DirectOpenedDb> {
    let input = nice_path(
        input
            .canonicalize()
            .with_context(|| format!("input file not found: {}", input.display()))?,
    );
    let filename = input
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("invalid filename"))?;

    let (kind, uid) = resolve_kind_uid(filename, uid_override)?;
    let password = passphrase_for_uid(&uid);

    let conn = Connection::open_with_flags(
        &input,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open failed: {}", input.display()))?;

    conn.execute_batch("PRAGMA query_only = ON; PRAGMA busy_timeout = 5000;")?;
    apply_v3_pragmas(&conn, &password)?;
    verify_open(&conn)?;

    Ok(DirectOpenedDb { conn, kind, uid })
}
