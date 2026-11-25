//! MDReader 集成测试入口。
//!
//! - 校验 `CARGO_BIN_EXE_MDReader` 二进制产物与 M1/M2 非阻塞 CLI 输出；
//! - 通过 `mdreader::core` 白盒联调 `config`、`storage`、`markdown` 与 `highlight` 协同闭环。

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use mdreader::core::config::{
    AppConfig, ConfigSaveError, ThemeMode, load_config, resolve_config_path_with_appdata,
    save_window_geometry_if_valid,
};
use mdreader::core::markdown::{BlockKind, TableAlign, TokenKind, parse_markdown};
use mdreader::core::storage::{
    DocumentEncodingMeta, LineEnding, StorageError, load_document, save_document_atomic,
};

static INTEGRATION_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct IntegrationTempDir {
    path: PathBuf,
}

impl IntegrationTempDir {
    fn new(label: &str) -> Self {
        let seq = INTEGRATION_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let dir =
            std::env::temp_dir().join(format!("mdreader_integration_test_{label}_{pid}_{seq}"));
        fs::create_dir_all(&dir).expect("应能创建集成测试临时目录");
        Self { path: dir }
    }
}

impl Drop for IntegrationTempDir {
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
fn test_binary_artifact_exists_and_non_empty() {
    let bin_path = Path::new(env!("CARGO_BIN_EXE_MDReader"));
    assert!(bin_path.exists(), "未找到构建产物: {}", bin_path.display());
    let meta = bin_path.metadata().expect("应能读取二进制文件元数据");
    assert!(meta.len() > 0, "二进制产物字节数不应为 0");
}

#[test]
fn test_cli_help_exits_cleanly_with_expected_output() {
    let bin_path = env!("CARGO_BIN_EXE_MDReader");
    let output = Command::new(bin_path)
        .arg("--help")
        .output()
        .expect("执行 MDReader.exe --help 失败");

    assert!(
        output.status.success(),
        "--help 应以退出码 0 返回，实际状态: {:?}",
        output.status
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("MDReader"));
    assert!(stdout.contains(".md"));
    assert!(stdout.contains("--register"));
}

#[test]
fn test_invalid_cli_flag_exits_with_code_2_and_stderr() {
    let bin_path = env!("CARGO_BIN_EXE_MDReader");
    let output = Command::new(bin_path)
        .arg("--unknown-flag-should-fail")
        .output()
        .expect("执行非法参数 MDReader.exe 失败");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("未知参数") || stderr.contains("--help"));
}

#[test]
fn test_core_config_priority_tolerance_and_geometry_protection_flow() {
    let dir = IntegrationTempDir::new("config_flow");
    let exe_dir = dir.path.join("exe");
    let appdata_dir = dir.path.join("appdata");
    fs::create_dir_all(&exe_dir).unwrap();
    fs::create_dir_all(appdata_dir.join("MDReader")).unwrap();

    // 便携配置覆盖 %APPDATA%，并带 BOM + 部分坏字段 + 用户自定义扩展键
    let portable_cfg = exe_dir.join("config.json");
    let appdata_cfg = appdata_dir.join("MDReader").join("config.json");
    fs::write(&appdata_cfg, r#"{"theme":"light"}"#).unwrap();
    fs::write(
        &portable_cfg,
        "\u{FEFF}{\n  \"theme\": \"dark\",\n  \"fontSize\": 999,\n  \"maxWidth\": 840,\n  \"userTag\": \"keep\"\n}\n",
    )
    .unwrap();

    let resolved = resolve_config_path_with_appdata(None, &exe_dir, Some(&appdata_dir));
    assert_eq!(resolved, portable_cfg);

    let loaded = load_config(Some(&resolved));
    assert!(!loaded.source_is_corrupt);
    assert!(loaded.warning.is_some());
    assert_eq!(loaded.config.theme, ThemeMode::Dark);
    assert_eq!(loaded.config.font_size, AppConfig::default().font_size);
    assert_eq!(loaded.config.max_width, 840);

    // 局部保存窗口几何，不丢失原有字段与用户扩展键
    let saved = save_window_geometry_if_valid(&loaded, &resolved, 1280, 800)
        .expect("合法对象配置应允许局部更新窗口几何");
    assert!(saved);
    let raw_after = fs::read_to_string(&resolved).unwrap();
    assert!(raw_after.contains("\"userTag\": \"keep\""));
    assert!(raw_after.contains("\"windowWidth\": 1280"));
    assert!(raw_after.contains("\"windowHeight\": 800"));

    // 整文件损坏时严禁写回覆写
    let corrupt_cfg = dir.path.join("corrupt.json");
    let corrupt_raw = b"[not-an-object";
    fs::write(&corrupt_cfg, corrupt_raw).unwrap();
    let corrupt_loaded = load_config(Some(&corrupt_cfg));
    assert!(corrupt_loaded.source_is_corrupt);
    let err = save_window_geometry_if_valid(&corrupt_loaded, &corrupt_cfg, 1024, 768).unwrap_err();
    assert!(matches!(err, ConfigSaveError::CorruptConfigProtected));
    assert_eq!(fs::read(&corrupt_cfg).unwrap(), corrupt_raw);
}

#[test]
fn test_core_storage_and_markdown_roundtrip_edit_save_reparse() {
    let dir = IntegrationTempDir::new("doc_roundtrip");
    let doc_path = dir.path.join("笔记 (v1).md");

    // 初始磁盘文件带 UTF-8 BOM 与 CRLF
    let mut initial_bytes = vec![0xEF, 0xBB, 0xBF];
    initial_bytes.extend_from_slice("# 初稿标题\r\n\r\n- [ ] 待办任务\r\n".as_bytes());
    fs::write(&doc_path, &initial_bytes).unwrap();

    let loaded = load_document(&doc_path).expect("应成功加载初始文档");
    assert!(loaded.meta.has_utf8_bom);
    assert_eq!(loaded.meta.line_ending, LineEnding::Crlf);
    assert!(!loaded.meta.was_lossy_utf8);
    assert_eq!(loaded.text, "# 初稿标题\n\n- [ ] 待办任务\n");

    let initial_parsed = parse_markdown(&loaded.text);
    assert_eq!(
        initial_parsed.blocks[0].kind,
        BlockKind::Heading { level: 1 }
    );

    // 模拟编辑区传入含混合换行的修订文本并原子保存
    let edited_text = "# 修订后标题\r\n\r\n- [x] 已完成任务\n\n| 列A | 列B |\n| :--- | ---: |\n| 1 | `ok` |\n\n```rust\nfn demo() { let n = 1..5; }\n```\n";
    save_document_atomic(&doc_path, edited_text, &loaded.meta).expect("应成功原子写回");

    // 验证磁盘字节完整保留 BOM、CRLF 且绝无 \r\r\n
    let saved_bytes = fs::read(&doc_path).unwrap();
    assert!(saved_bytes.starts_with(&[0xEF, 0xBB, 0xBF]));
    assert!(!saved_bytes.windows(3).any(|w| w == b"\r\r\n"));

    // 重读并重解析验证最新渲染数据
    let reloaded = load_document(&doc_path).unwrap();
    assert!(reloaded.meta.has_utf8_bom);
    assert_eq!(reloaded.meta.line_ending, LineEnding::Crlf);

    let reparsed = parse_markdown(&reloaded.text);
    assert!(matches!(
        reparsed.blocks[0].kind,
        BlockKind::Heading { level: 1 }
    ));
    assert_eq!(reparsed.blocks[0].spans[0].text, "修订后标题");
    assert!(reparsed.blocks.iter().any(|b| matches!(
        b.kind,
        BlockKind::ListItem {
            task: Some(true),
            ..
        }
    )));
    assert!(reparsed.blocks.iter().any(|b| matches!(
        &b.kind,
        BlockKind::Table { alignments, rows }
            if alignments == &vec![TableAlign::Left, TableAlign::Right] && rows.len() == 2
    )));

    let code_block = reparsed
        .blocks
        .iter()
        .find(|b| matches!(&b.kind, BlockKind::CodeBlock { language } if language.as_deref() == Some("rust")))
        .expect("应存在 Rust 代码块");
    let reconstructed: String = code_block.spans.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(reconstructed, "fn demo() { let n = 1..5; }");
    assert!(
        code_block
            .spans
            .iter()
            .any(|s| s.highlight == Some(TokenKind::Keyword) && s.text == "fn")
    );
}

#[test]
fn test_core_storage_readonly_protection_preserves_original_and_cleans_temp() {
    let dir = IntegrationTempDir::new("readonly_protection");
    let doc_path = dir.path.join("protected.md");
    let original = b"# Immutable Document\n";
    fs::write(&doc_path, original).unwrap();

    let mut perms = fs::metadata(&doc_path).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&doc_path, perms).unwrap();

    let err = save_document_atomic(&doc_path, "# Should Fail\n", &DocumentEncodingMeta::default())
        .expect_err("只读目标必须拒绝写入");
    match err {
        StorageError::Io(io_err) => assert_eq!(io_err.kind(), ErrorKind::PermissionDenied),
        other => panic!("期望 StorageError::Io(PermissionDenied)，实际: {other:?}"),
    }

    assert_eq!(fs::read(&doc_path).unwrap(), original);
    let entries: Vec<PathBuf> = fs::read_dir(&dir.path)
        .unwrap()
        .filter_map(|e| e.ok().map(|item| item.path()))
        .collect();
    assert_eq!(entries, vec![doc_path]);
}
