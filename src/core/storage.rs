//! 原子文件存储与编码/换行符嗅探模块。
//!
//! 负责文档的 UTF-8 BOM 与 LF/CRLF 探测、内部 `\n` 规范化、
//! 以及基于“同目录临时文件 + `FlushFileBuffers` + `ReplaceFileW`（仅目标缺失时回退 `MoveFileExW`）”
//! 的原子落盘机制，确保写入失败时原文件毫发无损且不遗留临时文件。

use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
use windows::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, REPLACEFILE_WRITE_THROUGH,
    ReplaceFileW,
};
use windows::core::PCWSTR;

/// UTF-8 BOM 字节序标记序列。
const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

/// 临时文件命名防碰撞自增序号。
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 创建唯一临时文件的最大冲突重试次数。
const MAX_TEMP_CREATE_ATTEMPTS: usize = 16;

/// 文档主换行符类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    /// Unix / 现代文本标准换行符（`\n`）。
    #[default]
    Lf,
    /// Windows 标准换行符（`\r\n`）。
    Crlf,
}

/// 文档编码与换行元信息（用于在修改保存时无损还原原文件格式特征）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DocumentEncodingMeta {
    /// 原文件开头是否包含 UTF-8 BOM（`0xEF 0xBB 0xBF`）。
    pub has_utf8_bom: bool,
    /// 原文件探测到的主换行符风格。
    pub line_ending: LineEnding,
    /// 读取原文件时是否因包含非法 UTF-8 字节而执行了有损替换。
    pub was_lossy_utf8: bool,
}

/// 从磁盘加载并规范化后的文档对象。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedDocument {
    /// 目标文档路径。
    pub path: PathBuf,
    /// 内部统一规范化为 `\n` 的文档文本内容。
    pub text: String,
    /// 原始编码与换行元信息。
    pub meta: DocumentEncodingMeta,
}

/// 存储读写操作错误类型。
#[derive(Debug)]
pub enum StorageError {
    /// 底层系统 I/O 或 Win32 替换失败错误。
    Io(io::Error),
    /// 目标路径非法（如空路径、目录路径或包含空字符）。
    InvalidPath(String),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "文件读写错误: {err}"),
            Self::InvalidPath(msg) => write!(f, "无效文件路径: {msg}"),
        }
    }
}

impl Error for StorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::InvalidPath(_) => None,
        }
    }
}

impl From<io::Error> for StorageError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// 临时文件 RAII 守卫：在未解除武装前，离开作用域时自动删除临时文件。
struct TempFileGuard {
    path: PathBuf,
    armed: bool,
}

impl TempFileGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// 校验文件路径合法性，拦截空路径、尾随分隔符路径、空字符及目录路径。
fn validate_file_path(path: &Path) -> Result<(), StorageError> {
    if path.as_os_str().is_empty() {
        return Err(StorageError::InvalidPath("文件路径不能为空".to_string()));
    }

    let mut last_unit = None;
    for unit in path.as_os_str().encode_wide() {
        if unit == 0 {
            return Err(StorageError::InvalidPath(
                "文件路径不能包含 NUL 空字符".to_string(),
            ));
        }
        last_unit = Some(unit);
    }

    if matches!(last_unit, Some(u) if u == u16::from(b'/') || u == u16::from(b'\\')) {
        return Err(StorageError::InvalidPath(format!(
            "路径以目录分隔符结尾，不能作为文件路径: {}",
            path.display()
        )));
    }

    if path.file_name().is_none() {
        return Err(StorageError::InvalidPath(format!(
            "路径缺少文件名部分: {}",
            path.display()
        )));
    }

    if let Ok(meta) = fs::metadata(path)
        && meta.is_dir()
    {
        return Err(StorageError::InvalidPath(format!(
            "目标路径是目录而非文件: {}",
            path.display()
        )));
    }

    Ok(())
}

/// 将路径编码为以 `\0` 结尾的 UTF-16 切片。
fn to_wide_null(path: &Path) -> Result<Vec<u16>, StorageError> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(StorageError::InvalidPath(
            "文件路径包含非法 NUL 字符".to_string(),
        ));
    }
    wide.push(0);
    Ok(wide)
}

/// 将 Win32 错误解包为标准 `std::io::Error`（保证 `PermissionDenied` 等 `ErrorKind` 精确）。
fn win32_error_to_io(err: windows::core::Error) -> io::Error {
    if let Some(code) = WIN32_ERROR::from_error(&err) {
        io::Error::from_raw_os_error(code.0 as i32)
    } else {
        io::Error::from(err)
    }
}

/// 探测原始文本字节流的主换行符风格。
fn detect_line_ending(bytes: &[u8]) -> LineEnding {
    let mut crlf_count = 0usize;
    let mut lf_count = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                if bytes.get(i + 1) == Some(&b'\n') {
                    crlf_count += 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            b'\n' => {
                lf_count += 1;
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    if crlf_count > 0 && crlf_count >= lf_count {
        LineEnding::Crlf
    } else {
        LineEnding::Lf
    }
}

/// 将外部文本的换行符（`\r\n` 与孤立 `\r`）统一规范化为内部 `\n`。
fn normalize_line_endings_to_lf(input: &str) -> String {
    if !input.contains('\r') {
        return input.to_string();
    }

    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut last = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == b'\r' {
            out.push_str(&input[last..i]);
            out.push('\n');
            if bytes.get(i + 1) == Some(&b'\n') {
                i += 2;
            } else {
                i += 1;
            }
            last = i;
        } else {
            i += 1;
        }
    }

    out.push_str(&input[last..]);
    out
}

/// 依据文档编码元信息重建输出字节流（单遍扫描换行，严防 `\r\r\n` 双重回车）。
fn encode_document_bytes(text: &str, meta: DocumentEncodingMeta) -> Vec<u8> {
    let bom_len = if meta.has_utf8_bom { UTF8_BOM.len() } else { 0 };

    if !text.contains('\r') && meta.line_ending == LineEnding::Lf {
        let mut out = Vec::with_capacity(bom_len + text.len());
        if meta.has_utf8_bom {
            out.extend_from_slice(&UTF8_BOM);
        }
        out.extend_from_slice(text.as_bytes());
        return out;
    }

    let extra = if meta.line_ending == LineEnding::Crlf {
        text.len() / 16 + 8
    } else {
        0
    };
    let mut out = Vec::with_capacity(bom_len + text.len() + extra);
    if meta.has_utf8_bom {
        out.extend_from_slice(&UTF8_BOM);
    }

    let eol: &[u8] = match meta.line_ending {
        LineEnding::Lf => b"\n",
        LineEnding::Crlf => b"\r\n",
    };

    let bytes = text.as_bytes();
    let mut last = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                out.extend_from_slice(&bytes[last..i]);
                out.extend_from_slice(eol);
                if bytes.get(i + 1) == Some(&b'\n') {
                    i += 2;
                } else {
                    i += 1;
                }
                last = i;
            }
            b'\n' => {
                out.extend_from_slice(&bytes[last..i]);
                out.extend_from_slice(eol);
                i += 1;
                last = i;
            }
            _ => {
                i += 1;
            }
        }
    }

    out.extend_from_slice(&bytes[last..]);
    out
}

/// 在目标同目录下以 `create_new(true)` 创建防碰撞唯一临时文件。
fn open_unique_temp_file(parent_dir: &Path) -> Result<(PathBuf, File), StorageError> {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);

    for _ in 0..MAX_TEMP_CREATE_ATTEMPTS {
        let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let file_name = format!(".mdreader.{pid}.{seq}.{nanos:x}.tmp");
        let temp_path = parent_dir.join(file_name);

        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(StorageError::Io(err)),
        }
    }

    Err(StorageError::Io(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "尝试多次后仍无法生成唯一临时文件",
    )))
}

/// 从磁盘加载 Markdown 文档并嗅探 UTF-8 BOM、主换行符及非 UTF-8 降级标志。
///
/// 支持 0 字节文件与仅含 3 字节 UTF-8 BOM 的空文档；内部 `text` 统一规范化为 `\n`。
pub fn load_document(path: &Path) -> Result<LoadedDocument, StorageError> {
    validate_file_path(path)?;

    let bytes = fs::read(path)?;
    let (has_utf8_bom, content_bytes) = match bytes.strip_prefix(&UTF8_BOM) {
        Some(rest) => (true, rest),
        None => (false, bytes.as_slice()),
    };

    let (raw_text, was_lossy_utf8) = match std::str::from_utf8(content_bytes) {
        Ok(valid) => (Cow::Borrowed(valid), false),
        Err(_) => (String::from_utf8_lossy(content_bytes), true),
    };

    let line_ending = detect_line_ending(raw_text.as_bytes());
    let text = if raw_text.contains('\r') {
        normalize_line_endings_to_lf(&raw_text)
    } else {
        raw_text.into_owned()
    };

    Ok(LoadedDocument {
        path: path.to_path_buf(),
        text,
        meta: DocumentEncodingMeta {
            has_utf8_bom,
            line_ending,
            was_lossy_utf8,
        },
    })
}

/// 按给定编码元信息将文本原子写回目标路径。
///
/// 写入顺序：重建 BOM 与目标换行字节流 -> 同目录唯一临时文件 -> `write_all` + `sync_all` (`FlushFileBuffers`)
/// -> 关闭句柄 -> `ReplaceFileW`（仅目标不存在返回 `ERROR_FILE_NOT_FOUND` 时回退 `MoveFileExW`）。
/// 遇到只读、占用或空间不足等错误时绝不回退覆盖，自动清理临时文件并保证原文件完好无损。
pub fn save_document_atomic(
    path: &Path,
    text: &str,
    meta: &DocumentEncodingMeta,
) -> Result<(), StorageError> {
    validate_file_path(path)?;
    let encoded = encode_document_bytes(text, *meta);
    write_bytes_atomic(path, &encoded)
}

/// 供 `core` 内部复用的纯字节流原子写盘原语（供 `save_document_atomic` 与 `config` 共用）。
pub(crate) fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
    validate_file_path(path)?;

    let target_w = to_wide_null(path)?;
    let parent_dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };

    let (temp_path, file) = open_unique_temp_file(parent_dir)?;
    // Rust 逆序析构规则：`temp_guard` 必须声明在 `file` 之前，
    // 保证无论正常还是错误提前返回，都会先关闭 `file` 句柄再由守卫尝试删除临时文件。
    let mut temp_guard = TempFileGuard::new(temp_path.clone());
    let temp_w = to_wide_null(&temp_path)?;

    {
        let mut file = file;
        file.write_all(bytes)?;
        file.sync_all()?;
    }

    // SAFETY: `target_w` 与 `temp_w` 均已校验为合法 NUL 结尾 UTF-16 路径指针，
    // 且临时文件句柄已在上一个作用域中完全关闭释放。
    let replace_res = unsafe {
        ReplaceFileW(
            PCWSTR(target_w.as_ptr()),
            PCWSTR(temp_w.as_ptr()),
            PCWSTR::null(),
            REPLACEFILE_WRITE_THROUGH,
            None,
            None,
        )
    };

    match replace_res {
        Ok(()) => {
            temp_guard.disarm();
            Ok(())
        }
        Err(win_err) => {
            if WIN32_ERROR::from_error(&win_err) == Some(ERROR_FILE_NOT_FOUND) {
                // SAFETY: `target_w` 与 `temp_w` 均为合法 NUL 结尾 UTF-16 路径指针，
                // 且仅在 `ReplaceFileW` 明确指示目标文件尚不存在时执行新建替换。
                unsafe {
                    MoveFileExW(
                        PCWSTR(temp_w.as_ptr()),
                        PCWSTR(target_w.as_ptr()),
                        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                    )
                }
                .map_err(|e| StorageError::Io(win32_error_to_io(e)))?;

                temp_guard.disarm();
                Ok(())
            } else {
                Err(StorageError::Io(win32_error_to_io(win_err)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;
    use std::os::windows::fs::OpenOptionsExt;

    struct TestTempDir {
        path: PathBuf,
    }

    impl TestTempDir {
        fn new(label: &str) -> Self {
            let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let pid = std::process::id();
            let dir = std::env::temp_dir().join(format!("mdreader_storage_test_{label}_{pid}_{seq}"));
            fs::create_dir_all(&dir).expect("应能创建测试临时目录");
            Self { path: dir }
        }
    }

    impl Drop for TestTempDir {
        #[allow(clippy::permissions_set_readonly_false)]
        fn drop(&mut self) {
            if let Ok(entries) = fs::read_dir(&self.path) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if let Ok(mut perms) = fs::metadata(&p).map(|m| m.permissions())
                        && perms.readonly()
                    {
                        perms.set_readonly(false);
                        let _ = fs::set_permissions(&p, perms);
                    }
                }
            }
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn test_invalid_paths_rejected_with_invalid_path() {
        let dir = TestTempDir::new("invalid_paths");

        assert!(matches!(
            load_document(Path::new("")),
            Err(StorageError::InvalidPath(_))
        ));
        assert!(matches!(
            save_document_atomic(Path::new(""), "abc", &DocumentEncodingMeta::default()),
            Err(StorageError::InvalidPath(_))
        ));
        assert!(matches!(
            load_document(&dir.path),
            Err(StorageError::InvalidPath(_))
        ));
        assert!(matches!(
            save_document_atomic(&dir.path, "abc", &DocumentEncodingMeta::default()),
            Err(StorageError::InvalidPath(_))
        ));

        let trailing_slash = PathBuf::from(format!("{}\\", dir.path.display()));
        assert!(matches!(
            save_document_atomic(&trailing_slash, "abc", &DocumentEncodingMeta::default()),
            Err(StorageError::InvalidPath(_))
        ));
    }

    #[test]
    fn test_zero_byte_document_load_and_atomic_save() {
        let dir = TestTempDir::new("zero_byte");
        let file_path = dir.path.join("empty.md");
        fs::write(&file_path, b"").expect("应能创建空文件");

        let loaded = load_document(&file_path).expect("应能加载 0 字节文件");
        assert_eq!(loaded.text, "");
        assert!(!loaded.meta.has_utf8_bom);
        assert_eq!(loaded.meta.line_ending, LineEnding::Lf);
        assert!(!loaded.meta.was_lossy_utf8);

        // 先写入非空再原子写回 0 字节
        fs::write(&file_path, b"non-empty").expect("写入前置内容");
        save_document_atomic(&file_path, "", &loaded.meta).expect("应能原子写回 0 字节");
        assert_eq!(fs::read(&file_path).expect("读取验证"), b"");
    }

    #[test]
    fn test_bom_only_file_load_and_save() {
        let dir = TestTempDir::new("bom_only");
        let file_path = dir.path.join("bom_empty.md");
        fs::write(&file_path, [0xEF, 0xBB, 0xBF]).expect("创建仅含 BOM 文件");

        let loaded = load_document(&file_path).expect("应能加载仅含 BOM 的文件");
        assert_eq!(loaded.text, "");
        assert!(loaded.meta.has_utf8_bom);
        assert_eq!(loaded.meta.line_ending, LineEnding::Lf);
        assert!(!loaded.meta.was_lossy_utf8);

        save_document_atomic(&file_path, "", &loaded.meta).expect("应保留 BOM 写回");
        assert_eq!(fs::read(&file_path).expect("读取验证"), vec![0xEF, 0xBB, 0xBF]);
    }

    #[test]
    fn test_bom_and_line_ending_sniffing_and_preservation() {
        let dir = TestTempDir::new("bom_eol");

        // 1. 有 BOM + CRLF
        let crlf_bom_path = dir.path.join("crlf_bom.md");
        let mut raw = vec![0xEF, 0xBB, 0xBF];
        raw.extend_from_slice("第一行\r\n第二行\r\n".as_bytes());
        fs::write(&crlf_bom_path, &raw).expect("写入测试文件");

        let loaded = load_document(&crlf_bom_path).expect("应成功加载");
        assert!(loaded.meta.has_utf8_bom);
        assert_eq!(loaded.meta.line_ending, LineEnding::Crlf);
        assert!(!loaded.meta.was_lossy_utf8);
        assert_eq!(loaded.text, "第一行\n第二行\n");

        save_document_atomic(&crlf_bom_path, &loaded.text, &loaded.meta).expect("原子写回");
        assert_eq!(fs::read(&crlf_bom_path).expect("读回字节"), raw);

        // 2. 无 BOM + LF 占优混合换行
        let mixed_lf_path = dir.path.join("mixed_lf.md");
        fs::write(&mixed_lf_path, b"a\nb\nc\r\nd\n").expect("写入混合换行");
        let loaded_lf = load_document(&mixed_lf_path).expect("加载混合换行");
        assert!(!loaded_lf.meta.has_utf8_bom);
        assert_eq!(loaded_lf.meta.line_ending, LineEnding::Lf);
        assert_eq!(loaded_lf.text, "a\nb\nc\nd\n");

        // 3. 混合换行但 CRLF 占优或持平 -> Crlf
        let mixed_eq_path = dir.path.join("mixed_eq.md");
        fs::write(&mixed_eq_path, b"a\r\nb\nc\r\nd\n").expect("写入持平换行");
        let loaded_eq = load_document(&mixed_eq_path).expect("加载持平换行");
        assert_eq!(loaded_eq.meta.line_ending, LineEnding::Crlf);
    }

    #[test]
    fn test_save_prevents_double_cr_in_crlf_and_lf_modes() {
        let dir = TestTempDir::new("no_double_cr");
        let path_crlf = dir.path.join("out_crlf.md");
        let path_lf = dir.path.join("out_lf.md");

        let dirty_input = "line1\r\nline2\rline3\nline4";

        let meta_crlf = DocumentEncodingMeta {
            has_utf8_bom: false,
            line_ending: LineEnding::Crlf,
            was_lossy_utf8: false,
        };
        save_document_atomic(&path_crlf, dirty_input, &meta_crlf).expect("CRLF 保存");
        let bytes_crlf = fs::read(&path_crlf).expect("读取 CRLF 输出");
        assert_eq!(bytes_crlf, b"line1\r\nline2\r\nline3\r\nline4");
        assert!(!bytes_crlf.windows(3).any(|w| w == b"\r\r\n"));

        let meta_lf = DocumentEncodingMeta {
            has_utf8_bom: false,
            line_ending: LineEnding::Lf,
            was_lossy_utf8: false,
        };
        save_document_atomic(&path_lf, dirty_input, &meta_lf).expect("LF 保存");
        let bytes_lf = fs::read(&path_lf).expect("读取 LF 输出");
        assert_eq!(bytes_lf, b"line1\nline2\nline3\nline4");
        assert!(!bytes_lf.contains(&b'\r'));
    }

    #[test]
    fn test_non_utf8_bytes_lossy_fallback_without_panic() {
        let dir = TestTempDir::new("non_utf8");
        let path = dir.path.join("gbk.md");
        // 含非法 UTF-8 字节序列（例如 GBK "中文" + 0xFF 0xFE）
        fs::write(&path, [b'#', b' ', 0xD6, 0xD0, 0xCE, 0xC4, 0xFF, 0xFE, b'\n'])
            .expect("写入非 UTF-8 字节");

        let loaded = load_document(&path).expect("非 UTF-8 应安全降级加载");
        assert!(loaded.meta.was_lossy_utf8);
        assert!(loaded.text.starts_with("# "));
        assert!(loaded.text.contains('\u{FFFD}'));
    }

    #[test]
    fn test_readonly_target_fails_with_permission_denied_and_no_temp_residue() {
        let dir = TestTempDir::new("readonly");
        let path = dir.path.join("readonly.md");
        let original = b"# Original Content\r\n";
        fs::write(&path, original).expect("创建原始文件");

        let mut perms = fs::metadata(&path).expect("读取权限").permissions();
        perms.set_readonly(true);
        fs::set_permissions(&path, perms).expect("设置只读权限");

        let meta = DocumentEncodingMeta {
            has_utf8_bom: false,
            line_ending: LineEnding::Crlf,
            was_lossy_utf8: false,
        };
        let err = save_document_atomic(&path, "# Overwritten\n", &meta)
            .expect_err("只读目标文件必须拒绝覆盖");

        match &err {
            StorageError::Io(io_err) => {
                assert_eq!(io_err.kind(), ErrorKind::PermissionDenied);
                assert!(err.source().is_some());
            }
            other => panic!("期望 StorageError::Io，实际为: {other:?}"),
        }

        // 校验原文件毫发无损
        assert_eq!(fs::read(&path).expect("读取原文件"), original);

        // 校验目录内仅存原文件，无任何 .tmp 残留
        let entries: Vec<PathBuf> = fs::read_dir(&dir.path)
            .expect("枚举目录")
            .filter_map(|e| e.ok().map(|item| item.path()))
            .collect();
        assert_eq!(entries, vec![path]);
    }

    #[test]
    fn test_locked_target_fails_without_corrupting_original_or_leaving_temp() {
        let dir = TestTempDir::new("locked");
        let path = dir.path.join("locked.md");
        let original = b"# Locked Original\n";
        fs::write(&path, original).expect("创建原文件");

        // 使用 share_mode(0) 模拟外部进程独占锁
        let _lock = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&path)
            .expect("应能以独占方式打开文件");

        let err = save_document_atomic(&path, "# Attempted Update\n", &DocumentEncodingMeta::default())
            .expect_err("独占锁定目标必须返回错误");
        assert!(matches!(err, StorageError::Io(_)));

        drop(_lock);

        assert_eq!(fs::read(&path).expect("原文件应毫发无损"), original);
        let entries: Vec<PathBuf> = fs::read_dir(&dir.path)
            .expect("枚举目录")
            .filter_map(|e| e.ok().map(|item| item.path()))
            .collect();
        assert_eq!(entries, vec![path]);
    }

    #[test]
    fn test_create_new_file_via_movefile_fallback() {
        let dir = TestTempDir::new("new_file");
        let path = dir.path.join("brand_new.md");
        assert!(!path.exists());

        let meta = DocumentEncodingMeta {
            has_utf8_bom: true,
            line_ending: LineEnding::Lf,
            was_lossy_utf8: false,
        };
        save_document_atomic(&path, "# 新建文档\n", &meta).expect("目标不存在时应回退 MoveFileExW 创建成功");

        let loaded = load_document(&path).expect("加载新建文件");
        assert!(loaded.meta.has_utf8_bom);
        assert_eq!(loaded.meta.line_ending, LineEnding::Lf);
        assert_eq!(loaded.text, "# 新建文档\n");
    }
}
