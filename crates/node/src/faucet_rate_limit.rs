//! PURPOSE: persistent per-IP cooldown store for the public HTTP faucet.
//!
//! INVARIANTS:
//! - Single-writer: owned by exactly one thread (the RPC server loop).
//!   No internal locking; concurrent writes are not protected against.
//! - Timestamps are UNIX seconds (u64). Instant cannot be persisted because
//!   it is monotonic and has no defined wall-clock conversion.
//! - Entries older than `COOLDOWN_SECS` are pruned on every load and on
//!   every write, so the on-disk file size stays bounded by the count of
//!   recently-active IPs.
//! - File writes are atomic: write to `<path>.tmp`, fsync, rename. A torn
//!   or unparseable file on load is treated as empty state and logged at
//!   warn level; a corrupted file never crashes the node.
//! - Persistence is best-effort. If a write fails (disk full, permission
//!   denied) the in-memory state is still updated so the live cooldown
//!   keeps working for the rest of the session.
//!
//! FAILURE MODES:
//! - File missing on first run: empty state, no error, info-level log.
//! - File unreadable / unparseable / unknown version: empty state, warn log.
//! - File write fails: in-memory state still updated, error log emitted.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Per-IP cooldown window for the public faucet (24 hours). Mirrors the
/// constant in rpc.rs by value so the persistence module can prune stale
/// entries without depending on rpc.rs internals.
const COOLDOWN_SECS: u64 = 24 * 3600;

/// On-disk schema version. Bumped only for breaking format changes; an
/// unknown version causes the file to be treated as empty rather than
/// silently misinterpreting old fields.
const DISK_VERSION: u32 = 1;

/// Wire format for the rate-limit map. Keys are `IpAddr::to_string()`
/// (e.g. `"127.0.0.1"`, `"::1"`) so the file is human-inspectable.
#[derive(Debug, Serialize, Deserialize)]
struct OnDisk {
    version: u32,
    entries: HashMap<String, u64>,
}

/// Persistent per-IP faucet cooldown store. See module docs for invariants.
pub struct FaucetRateLimit {
    path: PathBuf,
    entries: HashMap<IpAddr, u64>,
}

impl FaucetRateLimit {
    /// Open the store at `path`, loading existing state and pruning stale
    /// entries against `now_secs`. Missing, unreadable, unparseable, and
    /// unknown-version files all degrade to empty state without error.
    pub fn open(path: PathBuf, now_secs: u64) -> Self {
        let entries = load_entries(&path, now_secs);
        tracing::info!(
            path = %path.display(),
            entries = entries.len(),
            "Faucet rate-limit state loaded"
        );
        Self { path, entries }
    }

    /// Last successful dispense timestamp for `ip`, as UNIX seconds.
    pub fn last_dispense(&self, ip: IpAddr) -> Option<u64> {
        self.entries.get(&ip).copied()
    }

    /// Record a successful dispense at `now_secs`, prune entries that have
    /// aged out of the cooldown window, and persist the updated map to disk.
    /// Persistence failure is logged but never propagated: the in-memory
    /// state always reflects the new entry.
    pub fn record(&mut self, ip: IpAddr, now_secs: u64) {
        self.entries.insert(ip, now_secs);
        prune(&mut self.entries, now_secs);
        if let Err(e) = persist(&self.path, &self.entries) {
            tracing::error!(
                path = %self.path.display(),
                error = %e,
                "Failed to persist faucet rate-limit state"
            );
        }
    }

    /// Current count of tracked entries. Test-only visibility.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

fn load_entries(path: &Path, now_secs: u64) -> HashMap<IpAddr, u64> {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return HashMap::new();
        }
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "Faucet rate-limit file unreadable, starting empty"
            );
            return HashMap::new();
        }
    };
    let parsed: OnDisk = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "Faucet rate-limit file unparseable, starting empty"
            );
            return HashMap::new();
        }
    };
    if parsed.version != DISK_VERSION {
        tracing::warn!(
            path = %path.display(),
            version = parsed.version,
            expected = DISK_VERSION,
            "Faucet rate-limit file has unknown version, starting empty"
        );
        return HashMap::new();
    }
    let mut entries: HashMap<IpAddr, u64> = HashMap::new();
    for (k, v) in parsed.entries {
        if let Ok(ip) = k.parse::<IpAddr>() {
            entries.insert(ip, v);
        }
    }
    prune(&mut entries, now_secs);
    entries
}

fn prune(entries: &mut HashMap<IpAddr, u64>, now_secs: u64) {
    // saturating_sub: if a stored timestamp is somehow in the future (clock
    // skew between persist + load), the entry is still treated as fresh.
    entries.retain(|_, ts| now_secs.saturating_sub(*ts) < COOLDOWN_SECS);
}

fn persist(path: &Path, entries: &HashMap<IpAddr, u64>) -> std::io::Result<()> {
    let on_disk = OnDisk {
        version: DISK_VERSION,
        entries: entries
            .iter()
            .map(|(ip, ts)| (ip.to_string(), *ts))
            .collect(),
    };
    let json = serde_json::to_string(&on_disk).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("serialize faucet rate-limit: {e}"),
        )
    })?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let tmp_path = tmp_path_for(path);
    {
        let mut f = fs::File::create(&tmp_path)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp_path, path)?;
    Ok(())
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut s: OsString = path.as_os_str().to_os_string();
    s.push(".tmp");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Tiny RAII test directory under `std::env::temp_dir()` so the node
    /// crate does not take `tempfile` as a direct dev-dependency. Each
    /// instance gets a unique suffix from a process-static counter combined
    /// with the PID, and cleans up on drop.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let id = COUNTER.fetch_add(1, Ordering::SeqCst);
            let pid = std::process::id();
            let path = std::env::temp_dir().join(format!("novai_faucet_test_{label}_{pid}_{id}"));
            std::fs::create_dir_all(&path).expect("create test dir");
            TestDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn ipv4(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn now() -> u64 {
        1_700_000_000
    }

    #[test]
    fn open_missing_file_is_empty() {
        let dir = TestDir::new("missing");
        let path = dir.path().join("missing.json");
        let flr = FaucetRateLimit::open(path, now());
        assert_eq!(flr.len(), 0);
    }

    #[test]
    fn open_corrupt_file_is_empty_and_does_not_panic() {
        let dir = TestDir::new("corrupt");
        let path = dir.path().join("corrupt.json");
        fs::write(&path, b"this is not json").unwrap();
        let flr = FaucetRateLimit::open(path, now());
        assert_eq!(flr.len(), 0);
    }

    #[test]
    fn open_wrong_version_is_empty() {
        let dir = TestDir::new("wrongver");
        let path = dir.path().join("v999.json");
        fs::write(
            &path,
            br#"{"version":999,"entries":{"127.0.0.1":1700000000}}"#,
        )
        .unwrap();
        let flr = FaucetRateLimit::open(path, now());
        assert_eq!(flr.len(), 0);
    }

    #[test]
    fn record_persists_across_reopen() {
        let dir = TestDir::new("reopen");
        let path = dir.path().join("state.json");
        let ip = ipv4("1.2.3.4");

        {
            let mut flr = FaucetRateLimit::open(path.clone(), now());
            flr.record(ip, now());
            assert_eq!(flr.last_dispense(ip), Some(now()));
        }
        let flr = FaucetRateLimit::open(path, now() + 5);
        assert_eq!(flr.last_dispense(ip), Some(now()));
        assert_eq!(flr.len(), 1);
    }

    #[test]
    fn record_atomic_no_leftover_tmp_file() {
        let dir = TestDir::new("atomic");
        let path = dir.path().join("state.json");
        let ip = ipv4("1.2.3.4");

        let mut flr = FaucetRateLimit::open(path.clone(), now());
        flr.record(ip, now());

        let tmp = tmp_path_for(&path);
        assert!(!tmp.exists(), "tmp file must be renamed away after record");
        assert!(path.exists(), "final file must exist after record");
    }

    #[test]
    fn open_prunes_stale_entries() {
        let dir = TestDir::new("prune_open");
        let path = dir.path().join("state.json");
        let stale_ip = ipv4("1.2.3.4");
        let fresh_ip = ipv4("5.6.7.8");

        let stale_ts = now() - COOLDOWN_SECS - 10;
        let fresh_ts = now() - 60;
        let body = format!(
            r#"{{"version":1,"entries":{{"{}":{},"{}":{}}}}}"#,
            stale_ip, stale_ts, fresh_ip, fresh_ts,
        );
        fs::write(&path, body.as_bytes()).unwrap();

        let flr = FaucetRateLimit::open(path, now());
        assert_eq!(
            flr.last_dispense(stale_ip),
            None,
            "stale entry must be pruned"
        );
        assert_eq!(flr.last_dispense(fresh_ip), Some(fresh_ts));
        assert_eq!(flr.len(), 1);
    }

    #[test]
    fn record_prunes_stale_entries_during_write() {
        let dir = TestDir::new("prune_write");
        let path = dir.path().join("state.json");
        let stale_ip = ipv4("1.2.3.4");
        let new_ip = ipv4("5.6.7.8");

        {
            let mut flr = FaucetRateLimit::open(path.clone(), 0);
            flr.record(stale_ip, 0);
        }
        {
            let mut flr = FaucetRateLimit::open(path.clone(), COOLDOWN_SECS + 100);
            // Manually re-inject a stale entry to simulate the
            // pre-prune scenario where load could not prune (e.g. the wall
            // clock jumped backwards between persist and load). Then
            // record at the new time and confirm the on-write prune fires.
            flr.entries.insert(stale_ip, 0);
            flr.record(new_ip, COOLDOWN_SECS + 100);
            assert_eq!(flr.len(), 1, "stale entry must be pruned on write");
            assert_eq!(flr.last_dispense(stale_ip), None);
            assert_eq!(flr.last_dispense(new_ip), Some(COOLDOWN_SECS + 100));
        }
        let reopened = FaucetRateLimit::open(path, COOLDOWN_SECS + 100);
        assert_eq!(reopened.len(), 1);
    }

    #[test]
    fn record_ipv6_address_round_trips() {
        let dir = TestDir::new("ipv6");
        let path = dir.path().join("state.json");
        let ip = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1));
        {
            let mut flr = FaucetRateLimit::open(path.clone(), now());
            flr.record(ip, now());
        }
        let flr = FaucetRateLimit::open(path, now());
        assert_eq!(flr.last_dispense(ip), Some(now()));
    }

    #[test]
    fn record_multiple_ips_round_trip() {
        let dir = TestDir::new("multi");
        let path = dir.path().join("state.json");
        let ips: Vec<IpAddr> = (1u8..=10u8)
            .map(|i| IpAddr::V4(Ipv4Addr::new(10, 0, 0, i)))
            .collect();
        {
            let mut flr = FaucetRateLimit::open(path.clone(), now());
            for ip in &ips {
                flr.record(*ip, now());
            }
            assert_eq!(flr.len(), ips.len());
        }
        let flr = FaucetRateLimit::open(path, now());
        for ip in &ips {
            assert_eq!(flr.last_dispense(*ip), Some(now()));
        }
    }

    #[test]
    fn record_creates_parent_dir() {
        let dir = TestDir::new("parent");
        let path = dir.path().join("nested").join("subdir").join("state.json");
        let ip = ipv4("1.2.3.4");
        let mut flr = FaucetRateLimit::open(path.clone(), now());
        flr.record(ip, now());
        assert!(path.exists());
    }
}
