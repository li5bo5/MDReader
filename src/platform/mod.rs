//! Windows 平台集成层（归属 Milestone 3：`win32-platform`）。
//!
//! 本层独立于 `ui` 渲染细节，负责进程启动参数解析及无管理员权限的 Windows `.md` 文件关联管理。
//!
//! # 子模块说明
//! - `cli`：基于 `OsString` 的命令行参数解析（`CliAction`、`parse_args`），完整兼容含空格、
//!   中文与括号的文件路径，支持 `<file>`、`--config <path>`、`--register`、`--unregister`、`--help`，
//!   并提供兼容 GUI 子系统管道与父控制台的文本输出辅助。
//! - `assoc`：仅操作 `HKEY_CURRENT_USER`（HKCU）的四组 `.md`/`.markdown` 关联注册表写入与精准清理
//!   （`register_md_association`、`unregister_md_association`），严禁修改受保护的 `UserChoice` 键，
//!   并在操作后调用 `SHChangeNotify(SHCNE_ASSOCCHANGED, ...)` 通知系统壳层刷新。

pub mod assoc;
pub mod cli;

/// 平台集成层模块标识常量。
pub const LAYER_ID: &str = "platform";

/// 注册在 `HKCU\Software\Classes` 下的 MDReader ProgID 名称（跨轨道统一契约）。
pub const APP_PROG_ID: &str = "MDReader.Document";

/// 注册在 `HKCU\Software\RegisteredApplications` 与 `Applications` 下的应用标识名称。
pub const REGISTERED_APP_NAME: &str = "MDReader";

#[cfg(test)]
mod tests {
    use super::{APP_PROG_ID, LAYER_ID, REGISTERED_APP_NAME};

    #[test]
    fn test_platform_registry_identifiers_safe() {
        assert_eq!(LAYER_ID, "platform");
        for ident in [APP_PROG_ID, REGISTERED_APP_NAME] {
            assert!(!ident.is_empty());
            assert!(!ident.chars().any(char::is_whitespace));
            assert!(!ident.contains('"'));
            assert!(!ident.contains('\\'));
        }
    }
}
