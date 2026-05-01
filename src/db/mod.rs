//! Encrypted-DB plumbing: SQLCipher v3 parameters and passphrase derivation,
//! private workdir holding a copy of the source file, and the high-level
//! `open_encrypted` entry point used by the `query` and `shell` subcommands.

pub mod cipher;
pub mod open;
pub mod workdir;
