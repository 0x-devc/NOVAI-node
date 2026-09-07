//! Pin: the golden-vector regeneration gate must require `UPDATE_VECTORS=1`.
//!
//! Every golden-vector suite in this workspace branches on an environment
//! variable: when it is set to `1` the suite *rewrites* its `.bin` vectors from
//! the current encoder, otherwise it *compares* against the committed bytes.
//!
//! Four of the five gate sites once read `std::env::var("UPDATE_VECTORS").is_ok()`,
//! which is true for `UPDATE_VECTORS=0` and for the empty string, because
//! `is_ok()` only asks whether the variable is set at all. A run that merely had
//! the variable present in its environment therefore overwrote the golden
//! vectors and passed, so an encoding regression could not fail those suites.
//!
//! This is a STATIC check on source text, not a behavioural one: it proves the
//! gate expression is spelled correctly, not that a corrupted vector is caught
//! at run time. It fails loudly if it cannot find the gate sites at all, so a
//! rename of the variable cannot make it vacuously green.

use std::fs;
use std::path::{Path, PathBuf};

/// Directories under the workspace root that hold Rust source.
const SOURCE_ROOTS: [&str; 3] = ["crates", "tools", "sdk"];

/// The gate expression every site must use.
const REQUIRED: &str = r#"Some("1")"#;

/// The broken predicate that must not come back.
const FORBIDDEN: &str = ".is_ok()";

/// Number of gate sites known at the time this pin was written. The scan must
/// find at least this many, otherwise it is not looking at the real source and
/// its silence means nothing.
const MIN_GATE_SITES: usize = 5;

/// Lower bound on Rust files the walk must visit, for the same reason.
const MIN_FILES_SCANNED: usize = 100;

fn workspace_root() -> PathBuf {
    // crates/codec -> crates -> workspace root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("manifest dir has a workspace root two levels up")
        .to_path_buf()
}

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name == "target" || name == "node_modules" || name.starts_with('.') {
                continue;
            }
            collect_rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// A gate site: the file, the 1-based line number, and the code of that line
/// joined with the one after it, so a wrapped expression still reads whole.
struct GateSite {
    path: PathBuf,
    line_no: usize,
    text: String,
}

fn find_gate_sites() -> (Vec<GateSite>, usize) {
    let root = workspace_root();
    let mut files = Vec::new();
    for source_root in SOURCE_ROOTS {
        collect_rust_files(&root.join(source_root), &mut files);
    }
    files.sort();

    // This file quotes the needle in order to search for it, so it is not a gate
    // site and must be excluded. The assert keeps the exclusion honest: if the
    // path convention behind `file!()` ever changes, the skip would silently
    // stop matching and this pin would start reporting itself.
    let self_path = root.join(file!());
    assert!(
        self_path.is_file(),
        "cannot locate this pin's own source at {}; the self-exclusion below \
         would be a no-op",
        self_path.display()
    );

    let mut sites = Vec::new();
    for path in &files {
        if *path == self_path {
            continue;
        }
        let Ok(source) = fs::read_to_string(path) else {
            continue;
        };
        let lines: Vec<&str> = source.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            // Skip comments and doc comments: the usage instructions legitimately
            // mention the variable, and they are not gate expressions.
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            if !line.contains(r#"env::var("UPDATE_VECTORS")"#) {
                continue;
            }
            let mut text = (*line).to_string();
            if let Some(next) = lines.get(idx + 1) {
                text.push(' ');
                text.push_str(next.trim());
            }
            sites.push(GateSite {
                path: path
                    .strip_prefix(&root)
                    .unwrap_or(path.as_path())
                    .to_path_buf(),
                line_no: idx + 1,
                text,
            });
        }
    }
    (sites, files.len())
}

#[test]
fn update_vectors_gate_requires_the_literal_one() {
    let (sites, files_scanned) = find_gate_sites();

    assert!(
        files_scanned >= MIN_FILES_SCANNED,
        "the source walk visited only {files_scanned} Rust files under {SOURCE_ROOTS:?}, \
         which is too few to have reached the real source tree; this pin proves nothing \
         until the walk is fixed"
    );
    assert!(
        sites.len() >= MIN_GATE_SITES,
        "found only {} UPDATE_VECTORS gate sites, expected at least {MIN_GATE_SITES}; \
         either the variable was renamed or the scan is broken, and in both cases a \
         silent pass here would be meaningless",
        sites.len()
    );

    let mut offenders = Vec::new();
    for site in &sites {
        let ok = site.text.contains(REQUIRED) && !site.text.contains(FORBIDDEN);
        if !ok {
            offenders.push(format!(
                "  {}:{}  {}",
                site.path.display(),
                site.line_no,
                site.text.trim()
            ));
        }
    }

    assert!(
        offenders.is_empty(),
        "{} of {} UPDATE_VECTORS gate sites do not require the literal \"1\".\n\
         Each must read the variable and compare it to {REQUIRED}, so that \"0\", the \
         empty string, and any other value all mean COMPARE rather than REGENERATE.\n\
         Offending sites:\n{}",
        offenders.len(),
        sites.len(),
        offenders.join("\n")
    );
}
