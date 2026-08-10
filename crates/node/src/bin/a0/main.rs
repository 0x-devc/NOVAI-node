//! A0: the offline, read-only auditor for NOVAI state snapshots (gate F4).
//!
//! A0 is the first gate of the F4 snapshot series: it must exist and pass
//! before any snapshot is produced or installed. It operates ONLY on offline
//! copies of node data directories, never on a live data dir (RocksDB takes
//! a lock and may write housekeeping files on open, so pointing any A0 mode
//! at a running node's directory is forbidden by operating procedure).
//!
//! Subcommands:
//!   a0 valset                          print the dev validator set and quorum
//!   a0 inspect --db <copy-path>        report heights, roots, and QC voters
//!   a0 audit --db <copy-path> [--height <h>]
//!                                      run the full A1..A8 audit
//!
//! Exit codes: 0 = success / audit PASS, 1 = audit FAIL, 2 = usage or IO
//! error.
//!
//! Scope note (F4 execute gate, A0 only): no snapshot export, no install, no
//! E-pipeline. Those are a later gate; this binary only ever reads.
//!
//! The audit pipeline and its support modules live in the library at
//! `novai_node::snapshot` (gate F5 Stage 0), so the snapshot producer and
//! installer call this exact verifier instead of growing their own. This
//! binary is the CLI wrapper over them, and its behaviour is unchanged: same
//! subcommands, same flags, same report lines, same exit codes.

use novai_node::snapshot::{audit, inspect, valset};

fn usage() -> i32 {
    eprintln!("usage: a0 <valset|inspect|audit> [--db <path>] [--height <h>]");
    2
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn require_db(args: &[String]) -> Result<String, i32> {
    let Some(db) = flag_value(args, "--db") else {
        eprintln!("missing required --db <path>");
        return Err(usage());
    };
    let p = std::path::Path::new(&db);
    if !p.is_dir() {
        eprintln!("db path does not exist or is not a directory: {db}");
        return Err(2);
    }
    eprintln!("note: A0 must only be pointed at OFFLINE COPIES, never at a live node data dir");
    Ok(db)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("valset") => {
            valset::print_valset();
            0
        }
        Some("inspect") => match require_db(&args[1..]) {
            Ok(db) => match inspect::run(&db) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("inspect error: {e}");
                    2
                }
            },
            Err(code) => code,
        },
        Some("audit") => match require_db(&args[1..]) {
            Ok(db) => {
                let height = match flag_value(&args[1..], "--height") {
                    Some(raw) => match raw.parse::<u64>() {
                        Ok(h) => Some(h),
                        Err(_) => {
                            eprintln!("invalid --height value: {raw}");
                            std::process::exit(2);
                        }
                    },
                    None => None,
                };
                match audit::run(&db, height) {
                    Ok(true) => 0,
                    Ok(false) => 1,
                    Err(e) => {
                        eprintln!("audit error: {e}");
                        2
                    }
                }
            }
            Err(code) => code,
        },
        _ => usage(),
    };
    std::process::exit(code);
}
