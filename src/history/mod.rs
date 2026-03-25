use crate::{DebuggerError, Result};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunHistory {
    pub date: String,
    pub contract_hash: String,
    pub function: String,
    pub cpu_used: u64,
    pub memory_used: u64,
}

pub struct HistoryManager {
    file_path: PathBuf,
}

fn parse_history_date_to_utc_millis(date: &str) -> Option<i64> {
    let date = date.trim();
    if date.is_empty() {
        return None;
    }

    if let Ok(dt) = DateTime::parse_from_rfc3339(date) {
        return Some(dt.with_timezone(&Utc).timestamp_millis());
    }

    if let Ok(dt) = DateTime::parse_from_rfc2822(date) {
        return Some(dt.with_timezone(&Utc).timestamp_millis());
    }

    const FORMATS: &[&str] = &[
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%d",
        "%m/%d/%Y %H:%M:%S",
        "%m/%d/%Y %H:%M",
        "%m/%d/%Y",
        "%d/%m/%Y %H:%M:%S",
        "%d/%m/%Y %H:%M",
        "%d/%m/%Y",
    ];

    for fmt in FORMATS {
        if fmt.contains("%H") {
            if let Ok(naive) = NaiveDateTime::parse_from_str(date, fmt) {
                return Some(
                    DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc).timestamp_millis(),
                );
            }
        } else if let Ok(naive) = NaiveDate::parse_from_str(date, fmt) {
            let naive_dt = naive.and_hms_opt(0, 0, 0)?;
            return Some(
                DateTime::<Utc>::from_naive_utc_and_offset(naive_dt, Utc).timestamp_millis(),
            );
        }
    }

    None
}

fn compare_run_history_date(a: &RunHistory, b: &RunHistory) -> Ordering {
    match (
        parse_history_date_to_utc_millis(&a.date),
        parse_history_date_to_utc_millis(&b.date),
    ) {
        (Some(at), Some(bt)) => at.cmp(&bt),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.date.cmp(&b.date),
    }
}

/// Sorts records chronologically (oldest -> newest) using parsed timestamps when possible.
///
/// Records with unparseable dates are sorted after parseable ones, with a stable lexical fallback.
pub fn sort_records_by_date(records: &mut [RunHistory]) {
    records.sort_by(compare_run_history_date);
}

struct HistoryLockGuard {
    lock_path: PathBuf,
}

impl Drop for HistoryLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}

impl HistoryManager {
    /// Create a new HistoryManager using the default `~/.soroban-debug/history.json` path.
    pub fn new() -> Result<Self> {
        if let Ok(path) = std::env::var("SOROBAN_DEBUG_HISTORY_FILE") {
            let file_path = PathBuf::from(path);
            if let Some(parent) = file_path.parent() {
                if !parent.as_os_str().is_empty() && !parent.exists() {
                    fs::create_dir_all(parent).map_err(|e| {
                        DebuggerError::FileError(format!(
                            "Failed to create history directory {:?}: {}",
                            parent, e
                        ))
                    })?;
                }
            }
            return Ok(Self { file_path });
        }

        let home_dir = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map_err(|_| {
                DebuggerError::FileError("Could not determine home directory".to_string())
            })?;
        let debug_dir = PathBuf::from(home_dir).join(".soroban-debug");
        if !debug_dir.exists() {
            fs::create_dir_all(&debug_dir).map_err(|e| {
                DebuggerError::FileError(format!(
                    "Failed to create debug directory {:?}: {}",
                    debug_dir, e
                ))
            })?;
        }
        Ok(Self {
            file_path: debug_dir.join("history.json"),
        })
    }

    /// Create a new HistoryManager overriding the base path (for tests).
    pub fn with_path(path: PathBuf) -> Self {
        Self { file_path: path }
    }

    /// Read historical run data from disk.
    ///
    /// Returns `Err` if the history file exists but cannot be parsed.
    ///
    /// # Why we no longer silently return an empty `Vec`
    ///
    /// The previous implementation called
    /// `serde_json::from_reader(reader).unwrap_or_else(|_| Vec::new())`.
    /// That had two severe consequences:
    ///
    /// 1. **Silent data loss** — a corrupt or partially-written file appeared
    ///    identical to a brand-new installation.  Callers such as
    ///    `budget_trend_stats` and `check_regression` would operate on zero
    ///    records and produce misleading (or absent) output without any
    ///    indication that real data had been ignored.
    ///
    /// 2. **Destructive overwrite** — `append_record` calls `load_history`
    ///    before writing.  With the silent fallback it would receive an empty
    ///    `Vec`, push one new record, and atomically replace the corrupt file
    ///    with a single-record file, permanently destroying all prior history.
    ///
    /// Surfacing the error lets the caller decide on a recovery strategy and
    /// prevents any write path from silently clobbering salvageable data.
    pub fn load_history(&self) -> Result<Vec<RunHistory>> {
        if !self.file_path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&self.file_path).map_err(|e| {
            DebuggerError::FileError(format!(
                "Failed to open history file {:?}: {}",
                self.file_path, e
            ))
        })?;

        let reader = BufReader::new(file);

        // Surface parse failures rather than swallowing them.
        //
        // A corrupt or truncated file must not be treated as an empty history.
        // Doing so would cause `append_record` to overwrite the file with a
        // single-record list, destroying all salvageable data.
        let history: Vec<RunHistory> = serde_json::from_reader(reader).map_err(|e| {
            DebuggerError::FileError(format!(
                "History file \"{}\" could not be parsed ({}). \
                 The file may be corrupt or was written by an incompatible version. \
                 Recovery options:\n\
                 \x20 1. Inspect the file with `cat \"{}\"` and fix any JSON syntax errors.\n\
                 \x20 2. Back up and remove the file (`mv \"{}\" \"{}.bak\"`) to start fresh.\n\
                 \x20 3. Restore from a previous backup if one exists.",
                self.file_path.display(), e,
                self.file_path.display(),
                self.file_path.display(), self.file_path.display(),
            ))
        })?;
        Ok(history)
    }

    /// Append a new record optimizing with BufWriter.
    pub fn append_record(&self, record: RunHistory) -> Result<()> {
        let _lock = self.acquire_lock()?;
        let mut history = self.load_history()?;
        history.push(record);

        let tmp_path = self.file_path.with_extension("json.tmp");
        let file = File::create(&tmp_path).map_err(|e| {
            DebuggerError::FileError(format!(
                "Failed to create temp history file {:?}: {}",
                tmp_path, e
            ))
        })?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, &history).map_err(|e| {
            DebuggerError::FileError(format!(
                "Failed to write history file {:?}: {}",
                self.file_path, e
            ))
        })?;
        writer.flush().map_err(|e| {
            DebuggerError::FileError(format!(
                "Failed to flush temp history file {:?}: {}",
                tmp_path, e
            ))
        })?;
        if let Ok(file) = writer.into_inner() {
            let _ = file.sync_all();
        }

        if self.file_path.exists() {
            let _ = fs::remove_file(&self.file_path);
        }
        fs::rename(&tmp_path, &self.file_path).map_err(|e| {
            DebuggerError::FileError(format!(
                "Failed to replace history file {:?}: {}",
                self.file_path, e
            ))
        })?;
        Ok(())
    }

    /// Filter historical data based on optional parameters.
    pub fn filter_history(
        &self,
        contract_hash: Option<&str>,
        function: Option<&str>,
    ) -> Result<Vec<RunHistory>> {
        let history = self.load_history()?;
        let filtered = history
            .into_iter()
            .filter(|r| {
                let match_contract = match contract_hash {
                    Some(c) => r.contract_hash == c,
                    None => true,
                };
                let match_function = match function {
                    Some(f) => r.function == f,
                    None => true,
                };
                match_contract && match_function
            })
            .collect();
        Ok(filtered)
    }

    fn acquire_lock(&self) -> Result<HistoryLockGuard> {
        let lock_path = self.file_path.with_extension("lock");
        let start = SystemTime::now();

        loop {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(mut f) => {
                    let _ = writeln!(f, "pid={}", std::process::id());
                    return Ok(HistoryLockGuard { lock_path });
                }
                Err(_) => {
                    // If a previous process crashed, allow breaking a stale lock after a grace period.
                    if let Ok(meta) = fs::metadata(&lock_path) {
                        if let Ok(modified) = meta.modified() {
                            if modified
                                .elapsed()
                                .unwrap_or(Duration::from_secs(0))
                                .as_secs()
                                > 30
                            {
                                let _ = fs::remove_file(&lock_path);
                                continue;
                            }
                        }
                    }

                    if start.elapsed().unwrap_or(Duration::from_secs(0)).as_secs() > 30 {
                        return Err(DebuggerError::FileError(format!(
                            "Timed out waiting for history lock at {:?}",
                            lock_path
                        ))
                        .into());
                    }

                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
}

/// Calculate the delta between the last two runs. Returns percentage increase if >10%.
pub fn check_regression(records: &[RunHistory]) -> Option<(f64, f64)> {
    if records.len() < 2 {
        return None;
    }

    let mut sorted: Vec<&RunHistory> = records.iter().collect();
    sorted.sort_by(|a, b| compare_run_history_date(a, b));
    let latest = sorted[sorted.len() - 1];
    let previous = sorted[sorted.len() - 2];

    let mut regression_cpu = 0.0;
    let mut regression_mem = 0.0;

    if previous.cpu_used > 0 && latest.cpu_used > previous.cpu_used {
        let diff = (latest.cpu_used - previous.cpu_used) as f64;
        let p = (diff / previous.cpu_used as f64) * 100.0;
        if p > 10.0 {
            regression_cpu = p;
        }
    }

    if previous.memory_used > 0 && latest.memory_used > previous.memory_used {
        let diff = (latest.memory_used - previous.memory_used) as f64;
        let p = (diff / previous.memory_used as f64) * 100.0;
        if p > 10.0 {
            regression_mem = p;
        }
    }

    if regression_cpu > 0.0 || regression_mem > 0.0 {
        Some((regression_cpu, regression_mem))
    } else {
        None
    }
}

#[derive(Debug, Clone)]
pub struct BudgetTrendStats {
    pub count: usize,
    pub first_date: String,
    pub last_date: String,
    pub cpu_min: u64,
    pub cpu_avg: u64,
    pub cpu_max: u64,
    pub mem_min: u64,
    pub mem_avg: u64,
    pub mem_max: u64,
    pub last_cpu: u64,
    pub last_mem: u64,
}

pub fn budget_trend_stats(records: &[RunHistory]) -> Option<BudgetTrendStats> {
    if records.is_empty() {
        return None;
    }

    let mut cpu_min = u64::MAX;
    let mut cpu_max = 0u64;
    let mut mem_min = u64::MAX;
    let mut mem_max = 0u64;
    let mut cpu_sum: u128 = 0;
    let mut mem_sum: u128 = 0;

    for r in records {
        cpu_min = cpu_min.min(r.cpu_used);
        cpu_max = cpu_max.max(r.cpu_used);
        mem_min = mem_min.min(r.memory_used);
        mem_max = mem_max.max(r.memory_used);
        cpu_sum = cpu_sum.saturating_add(r.cpu_used as u128);
        mem_sum = mem_sum.saturating_add(r.memory_used as u128);
    }

    let mut sorted: Vec<&RunHistory> = records.iter().collect();
    sorted.sort_by(|a, b| compare_run_history_date(a, b));
    let count = sorted.len();
    let first = sorted[0];
    let last = sorted[count - 1];

    Some(BudgetTrendStats {
        count,
        first_date: first.date.clone(),
        last_date: last.date.clone(),
        cpu_min,
        cpu_avg: (cpu_sum / count as u128) as u64,
        cpu_max,
        mem_min,
        mem_avg: (mem_sum / count as u128) as u64,
        mem_max,
        last_cpu: last.cpu_used,
        last_mem: last.memory_used,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as IoWrite;
    use tempfile::NamedTempFile;
    use tempfile::TempDir;

    // ── helpers ─────────────────────────────────────────────────────────────

    fn make_record(date: &str, cpu: u64, mem: u64) -> RunHistory {
        RunHistory {
            date: date.into(),
            contract_hash: "hash".into(),
            function: "func".into(),
            cpu_used: cpu,
            memory_used: mem,
        }
    }



    // ── load_history — corrupt file tests (the requested verification) ───────

    /// A file containing invalid JSON must return `Err`, never `Ok(vec![])`.
    #[test]
    fn load_history_corrupt_file_returns_error() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"this is not json at all {{{").unwrap();
        tmp.flush().unwrap();

        let manager = HistoryManager::with_path(tmp.path().to_path_buf());
        let result = manager.load_history();

        assert!(
            result.is_err(),
            "corrupt file must return Err, not Ok(vec![])"
        );
    }

    /// The error message must reference the file path so the user knows which
    /// file to inspect.
    #[test]
    fn load_history_error_message_contains_file_path() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"{not valid json}").unwrap();
        tmp.flush().unwrap();

        let path = tmp.path().to_path_buf();
        let manager = HistoryManager::with_path(path.clone());
        let err = manager.load_history().unwrap_err();
        let msg = err.to_string();

        assert!(
            msg.contains(path.to_string_lossy().as_ref()),
            "error must mention the file path; got: {msg}"
        );
    }

    /// The error message must contain recovery guidance so users know what
    /// action to take without having to read source code.
    #[test]
    fn load_history_error_message_contains_recovery_guidance() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"[{\"broken\":").unwrap(); // truncated JSON
        tmp.flush().unwrap();

        let manager = HistoryManager::with_path(tmp.path().to_path_buf());
        let err = manager.load_history().unwrap_err();
        let msg = err.to_string();

        // The message must mention at least one concrete recovery action.
        let has_guidance = msg.contains("bak")      // backup suggestion
            || msg.contains("fix")                  // fix-in-place suggestion
            || msg.contains("Inspect")              // inspect suggestion
            || msg.contains("Recovery"); // recovery header

        assert!(
            has_guidance,
            "error must include recovery guidance; got: {msg}"
        );
    }

    /// A truncated (partial write) JSON array must also be treated as corrupt
    /// and return `Err`.
    #[test]
    fn load_history_truncated_file_returns_error() {
        let mut tmp = NamedTempFile::new().unwrap();
        // Valid start but cut off mid-record.
        tmp.write_all(b"[{\"date\":\"2026-01-01\",\"contract_hash\":\"a\"")
            .unwrap();
        tmp.flush().unwrap();

        let manager = HistoryManager::with_path(tmp.path().to_path_buf());
        assert!(
            manager.load_history().is_err(),
            "truncated file must return Err"
        );
    }

    /// A file containing a JSON object (not an array) must return `Err`
    /// because the expected type is `Vec<RunHistory>`.
    #[test]
    fn load_history_wrong_json_type_returns_error() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"{\"date\":\"2026\",\"cpu_used\":1}")
            .unwrap();
        tmp.flush().unwrap();

        let manager = HistoryManager::with_path(tmp.path().to_path_buf());
        assert!(
            manager.load_history().is_err(),
            "a JSON object instead of array must return Err"
        );
    }

    /// A non-existent file must still return `Ok(vec![])` (first-run case).
    #[test]
    fn load_history_missing_file_returns_empty_ok() {
        let path = std::env::temp_dir().join("soroban-history-definitely-does-not-exist.json");
        let _ = fs::remove_file(&path); // ensure absent
        let manager = HistoryManager::with_path(path);
        let result = manager.load_history();
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    /// A valid empty JSON array must parse successfully and return `Ok(vec![])`.
    #[test]
    fn load_history_empty_array_returns_ok() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"[]").unwrap();
        tmp.flush().unwrap();

        let manager = HistoryManager::with_path(tmp.path().to_path_buf());
        let result = manager.load_history().unwrap();
        assert!(result.is_empty());
    }

    /// `append_record` must propagate the error rather than silently overwriting
    /// a corrupt file with a single-record list.
    #[test]
    fn append_record_does_not_overwrite_corrupt_file() {
        let mut tmp = NamedTempFile::new().unwrap();
        let corrupt_content = b"this is corrupt {{{";
        tmp.write_all(corrupt_content).unwrap();
        tmp.flush().unwrap();
        let path = tmp.path().to_path_buf();

        let manager = HistoryManager::with_path(path.clone());
        let new_record = make_record("2026-01-01T00:00:00Z", 100, 200);

        let result = manager.append_record(new_record);
        assert!(
            result.is_err(),
            "append_record must fail on a corrupt history file, not silently overwrite it"
        );

        // The original corrupt content must still be on disk — not replaced.
        let on_disk = fs::read(&path).unwrap();
        assert_eq!(
            on_disk, corrupt_content,
            "corrupt file must not be modified by a failed append"
        );
    }

    // ── pre-existing tests (unchanged) ───────────────────────────────────────

    #[test]
    fn test_regression_detection() {
        let p1 = make_record("2026-01-01T00:00:00Z", 1000, 1000);
        let p2 = RunHistory {
            date: "2026-01-02T00:00:00Z".into(),
            contract_hash: "hash".into(),
            function: "func".into(),
            cpu_used: 1150,    // 15% increase
            memory_used: 1050, // 5% increase
        };

        let records = vec![p1, p2];
        let regression = check_regression(&records);
        assert!(regression.is_some());
        let (cpu, mem) = regression.unwrap();
        assert_eq!(cpu, 15.0);
        assert_eq!(mem, 0.0);
    }

    #[test]
    fn test_persistence_logic() {
        let temp = tempfile::tempdir().unwrap();
        let manager = HistoryManager::with_path(temp.path().join("history.json"));

        let record = make_record("2026-01-01T00:00:00Z", 1234, 5678);
        manager.append_record(record).unwrap();
        let history = manager.load_history().unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].cpu_used, 1234);
    }

    #[test]
    fn budget_trend_stats_empty_returns_none() {
        assert!(budget_trend_stats(&[]).is_none());
    }

    #[test]
    fn sort_records_by_date_handles_mixed_formats() {
        let mut records = vec![
            make_record("01/02/2026 00:00:00", 1, 1),
            make_record("2026-01-01T00:00:00Z", 1, 1),
            make_record("2026-01-03", 1, 1),
        ];

        sort_records_by_date(&mut records);
        assert_eq!(records[0].date, "2026-01-01T00:00:00Z");
        assert_eq!(records[1].date, "01/02/2026 00:00:00");
        assert_eq!(records[2].date, "2026-01-03");
    }

    #[test]
    fn budget_trend_stats_uses_parsed_date_order_for_first_last() {
        let records = vec![
            make_record("01/02/2026 00:00:00", 10, 10),
            make_record("2026-01-01T00:00:00Z", 20, 20),
        ];

        let stats = budget_trend_stats(&records).unwrap();
        assert_eq!(stats.first_date, "2026-01-01T00:00:00Z");
        assert_eq!(stats.last_date, "01/02/2026 00:00:00");
    }

    #[test]
    fn check_regression_uses_parsed_date_order_for_latest_two() {
        let records = vec![
            make_record("2026-01-02T00:00:00Z", 100, 100),
            make_record("01/03/2026 00:00:00", 120, 100),
            make_record("2026-01-01", 80, 100),
        ];

        let (cpu, mem) = check_regression(&records).unwrap();
        assert_eq!(cpu, 20.0);
        assert_eq!(mem, 0.0);
    }

    #[test]
    fn concurrent_append_preserves_all_records() {
        let temp = TempDir::new().unwrap();
        let history_path = temp.path().join("history.json");
        let manager = std::sync::Arc::new(HistoryManager::with_path(history_path));

        let threads = 16usize;
        let per_thread = 25usize;
        let mut handles = Vec::new();

        for t in 0..threads {
            let manager = std::sync::Arc::clone(&manager);
            handles.push(std::thread::spawn(move || {
                for i in 0..per_thread {
                    let record = RunHistory {
                        date: format!("t{t}-i{i}"),
                        contract_hash: "hash".into(),
                        function: "func".into(),
                        cpu_used: (t as u64) * 10 + i as u64,
                        memory_used: (t as u64) * 10 + i as u64,
                    };
                    manager.append_record(record).unwrap();
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let history = manager.load_history().unwrap();
        assert_eq!(history.len(), threads * per_thread);
    }

    #[test]
    fn new_respects_history_file_env_override() {
        let temp = TempDir::new().unwrap();
        let history_path = temp.path().join("custom").join("history.json");

        let old = std::env::var("SOROBAN_DEBUG_HISTORY_FILE").ok();
        std::env::set_var("SOROBAN_DEBUG_HISTORY_FILE", &history_path);

        let manager = HistoryManager::new().unwrap();
        manager
            .append_record(RunHistory {
                date: "d".into(),
                contract_hash: "h".into(),
                function: "f".into(),
                cpu_used: 1,
                memory_used: 1,
            })
            .unwrap();

        assert!(history_path.exists());

        if let Some(old) = old {
            std::env::set_var("SOROBAN_DEBUG_HISTORY_FILE", old);
        } else {
            std::env::remove_var("SOROBAN_DEBUG_HISTORY_FILE");
        }
    }

    #[test]
    fn budget_trend_stats_computes_min_max_avg_last() {
        let records = vec![
            make_record("2026-01-01T00:00:00Z", 10, 100),
            make_record("2026-01-02T00:00:00Z", 30, 200),
            make_record("2026-01-03T00:00:00Z", 20, 150),
        ];

        let stats = budget_trend_stats(&records).unwrap();
        assert_eq!(stats.count, 3);
        assert_eq!(stats.cpu_min, 10);
        assert_eq!(stats.cpu_max, 30);
        assert_eq!(stats.cpu_avg, 20);
        assert_eq!(stats.mem_min, 100);
        assert_eq!(stats.mem_max, 200);
        assert_eq!(stats.mem_avg, 150);
        assert_eq!(stats.last_cpu, 20);
        assert_eq!(stats.last_mem, 150);
        assert_eq!(stats.first_date, "2026-01-01T00:00:00Z");
        assert_eq!(stats.last_date, "2026-01-03T00:00:00Z");
    }
}
