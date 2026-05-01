//! Private scratch directory holding a copy of the encrypted DB and (for the
//! decrypt path) a staged plaintext output. Removed on Drop.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

/// Strip the Windows extended-length `\\?\` prefix from a canonicalized path
/// so display strings stay readable and SQLite ATTACH paths don't carry the
/// prefix. UNC paths (`\\?\UNC\...`) are left intact. No-op on non-Windows.
pub fn nice_path(p: PathBuf) -> PathBuf {
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

pub fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

pub struct WorkDir {
    pub root: PathBuf,
    pub db_copy: PathBuf,
    pub staged_plain: PathBuf,
}

impl WorkDir {
    /// Create a workdir under `parent_dir` and copy the encrypted DB (plus any
    /// `-wal` / `-shm` / `-journal` sidecars) into it.
    pub fn create(input: &Path, parent_dir: &Path) -> Result<Self> {
        let pid = std::process::id();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = parent_dir.join(format!(".aweme-db-decrypt-{pid}-{now}"));
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
