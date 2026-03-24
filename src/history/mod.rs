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

    /// Read historical data using highly optimized BufReader.
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
        let history: Vec<RunHistory> =
            serde_json::from_reader(reader).unwrap_or_else(|_| Vec::new());
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
        if let Some(file) = writer.into_inner().ok() {
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

                    if start.elapsed().unwrap_or(Duration::from_secs(0)).as_secs() > 5 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use tempfile::TempDir;

    #[test]
    fn test_regression_detection() {
        let p1 = RunHistory {
            date: "prev".into(),
            contract_hash: "hash".into(),
            function: "func".into(),
            cpu_used: 1000,
            memory_used: 1000,
        };
        let p2 = RunHistory {
            date: "latest".into(),
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
        let temp = NamedTempFile::new().unwrap();
        let manager = HistoryManager::with_path(temp.path().to_path_buf());

        let record = RunHistory {
            date: "date".into(),
            contract_hash: "hash".into(),
            function: "func".into(),
            cpu_used: 1234,
            memory_used: 5678,
        };

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
            RunHistory {
                date: "01/02/2026 00:00:00".into(),
                contract_hash: "hash".into(),
                function: "f".into(),
                cpu_used: 1,
                memory_used: 1,
            },
            RunHistory {
                date: "2026-01-01T00:00:00Z".into(),
                contract_hash: "hash".into(),
                function: "f".into(),
                cpu_used: 1,
                memory_used: 1,
            },
            RunHistory {
                date: "2026-01-03".into(),
                contract_hash: "hash".into(),
                function: "f".into(),
                cpu_used: 1,
                memory_used: 1,
            },
        ];

        sort_records_by_date(&mut records);
        assert_eq!(records[0].date, "2026-01-01T00:00:00Z");
        assert_eq!(records[1].date, "01/02/2026 00:00:00");
        assert_eq!(records[2].date, "2026-01-03");
    }

    #[test]
    fn budget_trend_stats_uses_parsed_date_order_for_first_last() {
        let records = vec![
            RunHistory {
                date: "01/02/2026 00:00:00".into(),
                contract_hash: "hash".into(),
                function: "f".into(),
                cpu_used: 10,
                memory_used: 10,
            },
            RunHistory {
                date: "2026-01-01T00:00:00Z".into(),
                contract_hash: "hash".into(),
                function: "f".into(),
                cpu_used: 20,
                memory_used: 20,
            },
        ];

        let stats = budget_trend_stats(&records).unwrap();
        assert_eq!(stats.first_date, "2026-01-01T00:00:00Z");
        assert_eq!(stats.last_date, "01/02/2026 00:00:00");
    }

    #[test]
    fn check_regression_uses_parsed_date_order_for_latest_two() {
        let records = vec![
            RunHistory {
                date: "2026-01-02T00:00:00Z".into(),
                contract_hash: "hash".into(),
                function: "f".into(),
                cpu_used: 100,
                memory_used: 100,
            },
            // This one is newest by date but inserted second.
            RunHistory {
                date: "01/03/2026 00:00:00".into(),
                contract_hash: "hash".into(),
                function: "f".into(),
                cpu_used: 120,
                memory_used: 100,
            },
            RunHistory {
                date: "2026-01-01".into(),
                contract_hash: "hash".into(),
                function: "f".into(),
                cpu_used: 80,
                memory_used: 100,
            },
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
    fn budget_trend_stats_computes_min_max_avg_last() {
        let records = vec![
            RunHistory {
                date: "2026-01-01T00:00:00Z".into(),
                contract_hash: "a".into(),
                function: "f".into(),
                cpu_used: 10,
                memory_used: 100,
            },
            RunHistory {
                date: "2026-01-02T00:00:00Z".into(),
                contract_hash: "a".into(),
                function: "f".into(),
                cpu_used: 30,
                memory_used: 200,
            },
            RunHistory {
                date: "2026-01-03T00:00:00Z".into(),
                contract_hash: "a".into(),
                function: "f".into(),
                cpu_used: 20,
                memory_used: 150,
            },
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
