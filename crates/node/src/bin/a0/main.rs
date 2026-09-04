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
//!   a0 verify-tree --db <copy-path>   prove the node store holds the tree its
//!                                      root claims; the ONE check `audit`
//!                                      cannot perform
//!   a0 reclaim --db <datadir> [--stage-only | --apply]
//!                                      census the dead SMT nodes; with
//!                                      --stage-only, rebuild and audit beside
//!                                      the directory without renaming; with
//!                                      --apply, rebuild and swap the directory
//!
//! Exit codes: 0 = success / audit PASS, 1 = audit FAIL, 2 = usage or IO
//! error.
//!
//! Scope note: `valset`, `inspect` and `audit` only ever read, and must be
//! pointed at OFFLINE COPIES. `reclaim` is the one exception in this binary and
//! it is the opposite case: it is pointed at a STOPPED node's own data
//! directory, because rebuilding it beside itself and renaming is the whole
//! point. Without `--apply` it still only reads. RocksDB's directory lock makes
//! the running-node mistake impossible rather than merely forbidden.
//!
//! The audit pipeline and its support modules live in the library at
//! `novai_node::snapshot` (gate F5 Stage 0), so the snapshot producer and
//! installer call this exact verifier instead of growing their own. This
//! binary is the CLI wrapper over them, and its behaviour is unchanged: same
//! subcommands, same flags, same report lines, same exit codes.

use novai_node::snapshot::{audit, inspect, reclaim, valset};

fn usage() -> i32 {
    eprintln!(
        "usage: a0 <valset|inspect|audit|verify-tree|reclaim> [--db <path>] [--height <h>] \
         [--stage-only] [--apply]"
    );
    2
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn db_path(args: &[String]) -> Result<String, i32> {
    let Some(db) = flag_value(args, "--db") else {
        eprintln!("missing required --db <path>");
        return Err(usage());
    };
    let p = std::path::Path::new(&db);
    if !p.is_dir() {
        eprintln!("db path does not exist or is not a directory: {db}");
        return Err(2);
    }
    Ok(db)
}

fn require_db(args: &[String]) -> Result<String, i32> {
    let db = db_path(args)?;
    eprintln!("note: A0 must only be pointed at OFFLINE COPIES, never at a live node data dir");
    Ok(db)
}

/// The reclaim target is a STOPPED node's own directory, so the offline-copy
/// note above would be actively misleading here. The note this prints instead
/// names the two things that matter to the operator running it.
fn require_datadir(args: &[String]) -> Result<String, i32> {
    let db = db_path(args)?;
    eprintln!(
        "note: reclaim targets a STOPPED node's own data directory and renames beside it; \
         the node must not be running"
    );
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
        // The census is the DEFAULT. `--apply` is the only thing that renames
        // anything, so an operator who mistypes the subcommand or forgets a
        // flag gets a report, never a swap.
        // Reads only, like `inspect` and `audit`, but it is pointed at a
        // STOPPED node's own directory as often as at a copy: the plan's 4.4
        // rollback is exactly the case where an operator has to decide between
        // the directory in place and the preserved one. So it gets its own
        // note rather than borrowing either of the two above.
        Some("verify-tree") => match db_path(&args[1..]) {
            Ok(db) => {
                eprintln!(
                    "note: verify-tree only reads; the node must not be running, and this is \
                     the one check `a0 audit` cannot perform"
                );
                match reclaim::run_verify_tree(&db) {
                    Ok(true) => 0,
                    Ok(false) => 1,
                    Err(e) => {
                        eprintln!("verify-tree error: {e}");
                        2
                    }
                }
            }
            Err(code) => code,
        },
        Some("reclaim") => match require_datadir(&args[1..]) {
            Ok(db) => {
                let apply = args[1..].iter().any(|a| a == "--apply");
                let stage_only = args[1..].iter().any(|a| a == "--stage-only");
                // Refused rather than resolved by precedence. The two flags ask
                // for opposite things about the one irreversible step in this
                // binary, so guessing which the operator meant is the wrong
                // service to offer.
                if apply && stage_only {
                    eprintln!(
                        "--apply and --stage-only are mutually exclusive: one swaps the \
                         directory, the other exists so you can decide first"
                    );
                    std::process::exit(2);
                }
                let mode = if apply {
                    reclaim::Mode::Apply
                } else if stage_only {
                    reclaim::Mode::StageOnly
                } else {
                    reclaim::Mode::DryRun
                };
                match reclaim::run(&db, mode) {
                    Ok(true) => 0,
                    Ok(false) => 1,
                    Err(e) => {
                        eprintln!("reclaim error: {e}");
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
