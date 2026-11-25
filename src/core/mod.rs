//! 核心无 GUI 业务逻辑层（归属 Milestone 2：`mdreader-core`）。
//!
//! 本层严禁依赖 `ui` 模块或任何 Direct2D / Win32 窗口控件句柄，
//! 确保所有配置解析、原子文件存储与 Markdown 解析逻辑可在 `cargo test` 下完全离线单测。
//!
//! # 子模块说明
//! - `config`：3 层配置路径查找（`--config` > `./config.json` > `%APPDATA%\MDReader\config.json`）、
//!   16 项核心字段与可选 `colors` 逐字段校验及安全回退（`ThemeMode`、`AppConfig`、`ConfigLoadResult`）。
//! - `storage`：UTF-8 BOM 与 LF/CRLF 嗅探、同卷临时文件 + `FlushFileBuffers` + `ReplaceFileW`
//!   （回退 `MoveFileExW`）原子写回原文件（`LineEnding`、`DocumentEncodingMeta`、`LoadedDocument`）。
//! - `markdown`：基于 `pulldown-cmark` 的流式 GFM 块模型解析（`BlockKind`、`RichSpan`、`ParsedDocument`、`parse_markdown`）。
//! - `highlight`：零正则多语言状态机词法高亮（`highlight_code`）。

pub mod config;
pub mod highlight;
pub mod history;
pub mod markdown;
pub mod storage;

/// 核心层模块标识常量。
pub const LAYER_ID: &str = "core";

/// 本地单一 JSON 配置文件默认文件名（借鉴极简无设置面板设计）。
pub const CONFIG_FILE_NAME: &str = "config.json";

/// 默认支持关联并打开的 Markdown 文件扩展名列表（小写含前导点）。
pub const SUPPORTED_EXTENSIONS: [&str; 2] = [".md", ".markdown"];

#[cfg(test)]
mod tests {
    use super::{CONFIG_FILE_NAME, LAYER_ID, SUPPORTED_EXTENSIONS};

    #[test]
    fn test_core_layer_constants_valid() {
        assert_eq!(LAYER_ID, "core");
        assert!(CONFIG_FILE_NAME.ends_with(".json"));
        assert!(!CONFIG_FILE_NAME.trim().is_empty());
    }

    #[test]
    fn test_supported_extensions_normalized() {
        assert!(SUPPORTED_EXTENSIONS.contains(&".md"));
        assert!(SUPPORTED_EXTENSIONS.contains(&".markdown"));
        for ext in SUPPORTED_EXTENSIONS {
            assert!(ext.starts_with('.'));
            assert_eq!(ext, ext.to_ascii_lowercase());
        }
    }
}
