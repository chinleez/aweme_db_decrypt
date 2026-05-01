use anyhow::Result;
use clap::{Parser, Subcommand};
use std::ffi::OsString;

mod cipher;
mod decrypt;
mod open;
mod output;
mod query;
mod shell;
mod sql_split;
mod workdir;

use decrypt::DecryptArgs;
use query::QueryArgs;
use shell::ShellArgs;

#[derive(Parser, Debug)]
#[command(
    name = "aweme-db-decrypt",
    version,
    about = "Decrypt and inspect IM databases shipped by com.ss.android.ugc.aweme.lite (抖音极速版)",
    long_about = "Decrypt and inspect encrypted IM SQLite databases of com.ss.android.ugc.aweme.lite. \
                  Supported filenames:\n  \
                  - encrypted_<uid>_im.db        (IM Core)\n  \
                  - encrypted_sub_<uid>_im.db    (IM Core, subprocess)\n  \
                  - encrypted_im_biz_<uid>.db    (IM Biz)\n\n\
                  Subcommands:\n  \
                  - decrypt   produce a plaintext SQLite copy on disk\n  \
                  - query     run SQL against the encrypted DB and print results\n  \
                  - shell     interactive REPL against the encrypted DB\n\n\
                  When the first positional argument is not a subcommand it is \
                  forwarded to `decrypt`, so prior usage \
                  (`aweme-db-decrypt <file>`) keeps working.\n\n\
                  Source files are never modified; all work happens on a private copy."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Decrypt the DB and write a plaintext SQLite copy to disk.
    Decrypt(DecryptArgs),
    /// Run SQL statements against the encrypted DB; print results to stdout.
    Query(QueryArgs),
    /// Interactive SQLite REPL against the encrypted DB.
    Shell(ShellArgs),
}

/// Inject `decrypt` as the implicit subcommand when the user invoked the binary
/// in the legacy form `aweme-db-decrypt <file>` (or `aweme-db-decrypt -u 123
/// file.db`). We treat the first arg as a subcommand only if it matches one of
/// the known names; everything else — file paths, flags, etc. — is treated as
/// legacy decrypt usage.
fn rewrite_argv(mut argv: Vec<OsString>) -> Vec<OsString> {
    // argv[0] is the binary name. Look at argv[1] to decide.
    let known: &[&str] = &[
        "decrypt", "query", "shell", "help", "-h", "--help", "-V", "--version",
    ];
    let needs_default = match argv.get(1).and_then(|s| s.to_str()) {
        None => false,            // bare invocation; let clap show its error
        Some(s) => !known.contains(&s),
    };
    if needs_default {
        argv.insert(1, OsString::from("decrypt"));
    }
    argv
}

fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Decrypt(a) => decrypt::run(a),
        Command::Query(a) => query::run(a),
        Command::Shell(a) => shell::run(a),
    }
}

fn main() {
    let argv = rewrite_argv(std::env::args_os().collect());
    let cli = Cli::parse_from(argv);
    if let Err(e) = dispatch(cli) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
