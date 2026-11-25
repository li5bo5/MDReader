//! 原生 Win32 / Direct2D / RichEdit UI 层（归属 Milestone 4 & Milestone 5）。
//!
//! 本层单向消费 `core` 提供的配置、文档编码与 Markdown 块模型，所有文件写入必须经由
//! `core::storage` 原子完成，禁止绕过核心层直接在 UI 事件中写盘。
//!
//! # 子模块说明
//! - `theme`：`Light` / `Dark` / `System`（读取 `AppsUseLightTheme`）调色板与自定义色值解析。
//! - `uia`：极轻量 UIA Override Provider，为原生 Win32 控件暴露语义化 `AutomationId`。
//! - `reader_view`：Direct2D / DirectWrite / WIC 居中限宽阅读画布（支持 `WM_PRINTCLIENT` 离屏截图）。
//! - `editor_view`：`Msftedit.dll`（`RICHEDIT50W`）纯文本编辑区、居中内边距与脏状态追踪。
//! - `window`：极简无设置主窗口、二态切换、`Ctrl+S` 原子保存自动回读、拖放/`Ctrl+O` 与热重载。

pub mod editor_view;
pub mod reader_view;
pub mod sidebar;
pub mod theme;
pub mod uia;
pub mod window;

/// 原生 UI 层模块标识常量。
pub const LAYER_ID: &str = "ui";

/// 应用展示名称与窗口标题后缀。
pub const APP_DISPLAY_NAME: &str = "MDReader";

/// 应用语义版本号（与 `Cargo.toml` 同步）。
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 官方 GitHub 仓库地址。
pub const APP_REPO_URL: &str = "https://github.com/li5bo5/MDReader";

/// 极简工具栏控件 ID：模式切换按钮（阅读态显示“编辑”，修改态显示“保存并阅读”）。
pub const IDC_MODE_TOGGLE: u16 = 1001;

/// 极简工具栏控件 ID：取消编辑并返回阅读模式按钮（仅修改态可见）。
pub const IDC_EDIT_CANCEL: u16 = 1002;

/// 极简工具栏控件 ID：打开本地 Markdown 文件按钮。
pub const IDC_FILE_OPEN: u16 = 1003;

/// 修改模式主编辑区控件 ID（标准 `Msftedit.dll` `RICHEDIT50W` 实例）。
pub const IDC_EDITOR: u16 = 1004;

/// 页内查找栏输入框控件 ID（`Ctrl+F` 唤起时可见）。
pub const IDC_FIND_INPUT: u16 = 1005;

/// 页内查找栏命中计数标签控件 ID（显示 `"1/3"` 或 `"0 命中"`）。
pub const IDC_FIND_STATUS: u16 = 1006;

/// 阅读模式 Direct2D 画布控件 ID。
pub const IDC_READER: u16 = 1007;

/// 左侧大纲/历史目录栏控件 ID。
pub const IDC_SIDEBAR: u16 = 1008;

/// 侧边栏点击大纲跳转通知消息（`WPARAM = block_index`, `LPARAM = start_line`）。
pub const WM_APP_SIDEBAR_OUTLINE_JUMP: u32 =
    windows::Win32::UI::WindowsAndMessaging::WM_APP + 10;

/// 侧边栏点击历史记录打开文件通知消息（`WPARAM = history_index`）。
pub const WM_APP_SIDEBAR_HISTORY_OPEN: u32 =
    windows::Win32::UI::WindowsAndMessaging::WM_APP + 11;

/// 阅读画布滚动位置变化通知消息（供主窗口同步更新大纲当前激活章节）。
pub const WM_APP_READER_SCROLLED: u32 =
    windows::Win32::UI::WindowsAndMessaging::WM_APP + 12;

/// 阅读器核心交互模式（阅读模式 ↔ 修改模式二态机）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppMode {
    /// 默认阅读模式（M4 Direct2D / DirectWrite 渲染画布）。
    #[default]
    Reading,
    /// 修改模式（M5 `Msftedit.dll` 纯文本编辑区）。
    Editing,
}

impl AppMode {
    /// 返回与窗口标题契约（`"<filename>[*] - <模式> - MDReader"`）一致的模式文案。
    pub const fn title_label(self) -> &'static str {
        match self {
            Self::Reading => "阅读模式",
            Self::Editing => "修改模式",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        APP_DISPLAY_NAME, AppMode, IDC_EDIT_CANCEL, IDC_EDITOR, IDC_FILE_OPEN, IDC_FIND_INPUT,
        IDC_FIND_STATUS, IDC_MODE_TOGGLE, IDC_READER, IDC_SIDEBAR, LAYER_ID,
    };

    #[test]
    fn test_app_mode_default_and_labels() {
        assert_eq!(AppMode::default(), AppMode::Reading);
        assert_eq!(AppMode::Reading.title_label(), "阅读模式");
        assert_eq!(AppMode::Editing.title_label(), "修改模式");
    }

    #[test]
    fn test_ui_control_ids_unique_and_nonzero() {
        let ids = [
            IDC_MODE_TOGGLE,
            IDC_EDIT_CANCEL,
            IDC_FILE_OPEN,
            IDC_EDITOR,
            IDC_FIND_INPUT,
            IDC_FIND_STATUS,
            IDC_READER,
            IDC_SIDEBAR,
        ];
        for (i, &id_a) in ids.iter().enumerate() {
            assert!(id_a > 0);
            for &id_b in &ids[(i + 1)..] {
                assert_ne!(id_a, id_b);
            }
        }
        assert_eq!(LAYER_ID, "ui");
        assert_eq!(APP_DISPLAY_NAME, "MDReader");
    }
}
