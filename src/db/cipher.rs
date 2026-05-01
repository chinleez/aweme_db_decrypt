//! SQLCipher v3 parameters and passphrase derivation for the aweme.lite IM
//! databases. The values here mirror what the WCDB 2 build inside the APK
//! actually writes to disk.

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::Connection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbKind {
    ImCore,
    ImBiz,
}

impl DbKind {
    pub fn schema_baseline(&self) -> i32 {
        match self {
            DbKind::ImCore => 73,
            DbKind::ImBiz => 56,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            DbKind::ImCore => "IM Core (encrypted_<uid>_im.db)",
            DbKind::ImBiz => "IM Biz  (encrypted_im_biz_<uid>.db)",
        }
    }
}

pub fn validate_uid(uid: &str) -> Result<()> {
    if uid.is_empty() {
        bail!("uid is empty");
    }
    if !uid.chars().all(|c| c.is_ascii_digit()) {
        bail!("uid must contain only digits, got {:?}", uid);
    }
    Ok(())
}

pub fn detect(filename: &str) -> Result<(DbKind, String)> {
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

pub fn passphrase_for_uid(uid: &str) -> String {
    const PREFIX: &str = "byte";
    const MIDDLE: &str = "imwcdb";
    const SUFFIX: &str = "dance";
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

pub fn apply_v3_pragmas(conn: &Connection, password: &str) -> Result<()> {
    conn.pragma_update(None, "key", password)
        .context("PRAGMA key")?;
    conn.pragma_update(None, "cipher_page_size", 4096i64)?;
    conn.pragma_update(None, "kdf_iter", 64000i64)?;
    conn.pragma_update(None, "cipher_use_hmac", 1i64)?;
    conn.pragma_update(None, "cipher_hmac_algorithm", "HMAC_SHA1")?;
    conn.pragma_update(None, "cipher_kdf_algorithm", "PBKDF2_HMAC_SHA1")?;
    Ok(())
}

pub fn verify_open(conn: &Connection) -> Result<i64> {
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
        .map_err(|e| anyhow!("decryption failed (wrong uid / wrong cipher params): {e}"))
}

/// Resolve the (DbKind, uid) pair from filename + optional override, mirroring
/// the precedence used by the original CLI: explicit --uid wins, with a note if
/// it disagrees with the filename; missing override falls back to detection.
pub fn resolve_kind_uid(filename: &str, override_uid: Option<&str>) -> Result<(DbKind, String)> {
    match override_uid {
        Some(u) => {
            validate_uid(u)?;
            match detect(filename) {
                Ok((k, detected)) => {
                    if detected != u {
                        eprintln!(
                            "[!] note: --uid {u} differs from filename uid {detected}; using --uid"
                        );
                    }
                    Ok((k, u.to_string()))
                }
                Err(_) => {
                    eprintln!("[i] note: filename does not match expected pattern; assuming IM Core");
                    Ok((DbKind::ImCore, u.to_string()))
                }
            }
        }
        None => detect(filename),
    }
}
