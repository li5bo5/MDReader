//! 最近打开文件历史持久化模块（默认保留 7 天，原子写盘，无 GUI 依赖）。

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::core::config::APP_CONFIG_DIR_NAME;
use crate::core::storage;

/// 历史记录默认保留时长：7 天（秒）。
pub const HISTORY_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;

/// 单次最多保留的历史记录条数。
pub const MAX_HISTORY_ENTRIES: usize = 64;

/// 历史记录文件名。
pub const HISTORY_FILE_NAME: &str = "history.json";

/// 单条文件打开历史。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub path: String,
    pub last_opened_secs: u64,
}

impl HistoryEntry {
    /// 返回用于界面展示的纯文件名（若提取失败则回退完整路径）。
    pub fn file_name(&self) -> &str {
        Path::new(&self.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&self.path)
    }
}

/// 7 天文件打开历史集合。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistory {
    pub entries: Vec<HistoryEntry>,
}

/// 返回当前 Unix 时间戳（秒）。
pub fn current_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 解析历史文件持久化路径（优先 `%APPDATA%\MDReader\history.json`，缺失时回退 `exe_dir`）。
pub fn resolve_history_path(exe_dir: &Path) -> PathBuf {
    if let Some(appdata) = env::var_os("APPDATA").filter(|v| !v.is_empty()) {
        return PathBuf::from(appdata)
            .join(APP_CONFIG_DIR_NAME)
            .join(HISTORY_FILE_NAME);
    }
    exe_dir.join(HISTORY_FILE_NAME)
}

impl FileHistory {
    /// 从磁盘加载历史记录；文件缺失或 JSON 损坏时安全返回空列表，并自动淘汰 7 天前记录。
    pub fn load(path: &Path, now_secs: u64) -> Self {
        let Ok(raw) = fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(mut history) = serde_json::from_str::<FileHistory>(&raw) else {
            return Self::default();
        };
        history.normalize(now_secs);
        history
    }

    /// 记录一次文件打开事件：去重置顶、清理 7 天前过期项并截断上限。
    pub fn record_open(&mut self, file_path: &Path, now_secs: u64) {
        let normalized_str = normalize_display_path(file_path);
        if normalized_str.is_empty() {
            return;
        }
        self.entries
            .retain(|e| !paths_equal_case_insensitive(&e.path, &normalized_str));
        self.entries.insert(
            0,
            HistoryEntry {
                path: normalized_str,
                last_opened_secs: now_secs,
            },
        );
        self.normalize(now_secs);
    }

    /// 按最近打开时间降序排列，过滤超过 7 天或空路径的条目并去重。
    pub fn normalize(&mut self, now_secs: u64) {
        self.entries.retain(|e| {
            !e.path.trim().is_empty()
                && now_secs.saturating_sub(e.last_opened_secs) <= HISTORY_RETENTION_SECS
        });
        self.entries
            .sort_by(|a, b| b.last_opened_secs.cmp(&a.last_opened_secs));

        let mut deduped: Vec<HistoryEntry> = Vec::with_capacity(self.entries.len());
        for entry in self.entries.drain(..) {
            if !deduped
                .iter()
                .any(|existing| paths_equal_case_insensitive(&existing.path, &entry.path))
            {
                deduped.push(entry);
            }
        }
        deduped.truncate(MAX_HISTORY_ENTRIES);
        self.entries = deduped;
    }

    /// 原子写回磁盘。
    pub fn save(&self, path: &Path) -> Result<(), storage::StorageError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        storage::write_bytes_atomic(path, &json)
    }
}

/// Windows 路径不区分大小写比较（同时兼容 Unicode 中文路径）。
fn paths_equal_case_insensitive(a: &str, b: &str) -> bool {
    if a.len() == b.len() && a.eq_ignore_ascii_case(b) {
        return true;
    }
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

/// 去除 Windows 扩展路径前缀 `\\?\` 以便于展示与去重。
fn normalize_display_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        stripped.to_string()
    } else {
        s.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_prunes_older_than_7_days_and_dedups() {
        let now = 1_700_000_000u64;
        let mut history = FileHistory::default();

        // 8 天前的记录应被淘汰
        history.entries.push(HistoryEntry {
            path: r"C:\docs\expired.md".to_string(),
            last_opened_secs: now - HISTORY_RETENTION_SECS - 1,
        });
        // 刚好 7 天内的记录应保留
        history.entries.push(HistoryEntry {
            path: r"C:\docs\valid.md".to_string(),
            last_opened_secs: now - HISTORY_RETENTION_SECS,
        });

        history.record_open(Path::new(r"C:\docs\VALID.md"), now - 10);
        history.record_open(Path::new(r"C:\docs\latest.md"), now);

        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.entries[0].path, r"C:\docs\latest.md");
        assert_eq!(history.entries[1].path, r"C:\docs\VALID.md");
    }

    #[test]
    fn test_history_load_corrupt_returns_empty_and_roundtrips_atomic() {
        let temp_dir = env::temp_dir().join(format!(
            "mdreader_history_test_{}_{}",
            std::process::id(),
            current_epoch_secs()
        ));
        fs::create_dir_all(&temp_dir).unwrap();
        let file = temp_dir.join("history.json");

        fs::write(&file, "{ not valid json").unwrap();
        let loaded = FileHistory::load(&file, 1_700_000_000);
        assert!(loaded.entries.is_empty());

        let mut h = FileHistory::default();
        h.record_open(Path::new(r"C:\notes\中文笔记.md"), 1_700_000_000);
        h.save(&file).unwrap();

        let reloaded = FileHistory::load(&file, 1_700_000_100);
        assert_eq!(reloaded.entries.len(), 1);
        assert_eq!(reloaded.entries[0].file_name(), "中文笔记.md");

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
