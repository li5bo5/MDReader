//! 命令行参数解析与 GUI/CLI 混合终端输出模块（归属 Milestone 3）。
//!
//! 特点：
//! - 全程基于 `OsString` / `PathBuf` 解析文件路径，无损兼容空格、中文、括号及非常规 UTF-16 路径；
//! - 支持 `<file>`、`--config <path>`、`--config=<path>`、`--register`、`--unregister`、`-h`/`--help` 以及 `--` 分隔符；
//! - 提供同时兼容重定向管道与父控制台附加的输出辅助，确保 `#![windows_subsystem = "windows"]` 下
//!   双击 `.md` 零黑框，而命令行执行 `--help` / `--register` / `--unregister` 依然能稳定输出文本。

use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{FILE_TYPE_CHAR, FILE_TYPE_UNKNOWN, GetFileType, WriteFile};
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE,
    STD_OUTPUT_HANDLE, WriteConsoleW,
};

use crate::core::{CONFIG_FILE_NAME, SUPPORTED_EXTENSIONS};
use crate::ui::{APP_DISPLAY_NAME, AppMode};

/// 解析后的命令行执行意图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliAction {
    /// 启动主窗口并进入默认阅读模式。
    OpenWindow {
        file_path: Option<PathBuf>,
        config_path: Option<PathBuf>,
    },
    /// 在当前用户（HKCU）注册 `.md` / `.markdown` 文件打开方式。
    RegisterAssoc,
    /// 从当前用户（HKCU）精准移除 MDReader 文件关联。
    UnregisterAssoc,
    /// 输出帮助文本后正常退出。
    ShowHelp,
}

/// 命令行参数解析错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub message: String,
}

impl CliError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for CliError {}

/// 判断 `OsStr` 是否以 ASCII 前缀开头，并返回剩余切片对应的 `OsString`。
fn strip_os_prefix(arg: &OsStr, prefix: &str) -> Option<OsString> {
    let arg_units: Vec<u16> = arg.encode_wide().collect();
    let prefix_units: Vec<u16> = prefix.encode_utf16().collect();
    if arg_units.len() >= prefix_units.len() && arg_units[..prefix_units.len()] == prefix_units[..] {
        use std::os::windows::ffi::OsStringExt;
        Some(OsString::from_wide(&arg_units[prefix_units.len()..]))
    } else {
        None
    }
}

/// 判断参数是否以 `-` 开头（仅当非空且首码元为 `'-'`）。
fn starts_with_dash(arg: &OsStr) -> bool {
    arg.encode_wide().next() == Some(u16::from(b'-'))
}

/// 解析不含程序名 `argv[0]` 的命令行参数序列。
///
/// 若直接传入 `std::env::args_os().skip(1)`，可无缝处理任意 Windows 文件路径。
pub fn parse_args<I>(args: I) -> Result<CliAction, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut iter = args.into_iter();
    let mut do_help = false;
    let mut do_register = false;
    let mut do_unregister = false;
    let mut config_path: Option<PathBuf> = None;
    let mut file_path: Option<PathBuf> = None;
    let mut positional_only = false;

    while let Some(arg) = iter.next() {
        if !positional_only {
            if let Some(s) = arg.to_str() {
                match s {
                    "-h" | "--help" => {
                        do_help = true;
                        continue;
                    }
                    "--register" => {
                        do_register = true;
                        continue;
                    }
                    "--unregister" => {
                        do_unregister = true;
                        continue;
                    }
                    "--config" => {
                        let Some(val) = iter.next() else {
                            return Err(CliError::new("参数错误: --config 缺少配置文件路径参数"));
                        };
                        if val.is_empty() {
                            return Err(CliError::new("参数错误: --config 路径不能为空"));
                        }
                        config_path = Some(PathBuf::from(val));
                        continue;
                    }
                    "--" => {
                        positional_only = true;
                        continue;
                    }
                    _ => {}
                }
            }

            if let Some(val) = strip_os_prefix(&arg, "--config=") {
                if val.is_empty() {
                    return Err(CliError::new("参数错误: --config= 路径不能为空"));
                }
                config_path = Some(PathBuf::from(val));
                continue;
            }

            if starts_with_dash(&arg) && arg != OsStr::new("-") {
                return Err(CliError::new(format!(
                    "未知参数: {}。请使用 --help 查看用法。",
                    arg.to_string_lossy()
                )));
            }
        }

        if file_path.is_some() {
            return Err(CliError::new(format!(
                "参数错误: 仅支持传入一个文件路径，多余参数: {}",
                arg.to_string_lossy()
            )));
        }
        if arg.is_empty() {
            return Err(CliError::new("参数错误: 文件路径不能为空"));
        }
        file_path = Some(PathBuf::from(arg));
    }

    if do_help {
        return Ok(CliAction::ShowHelp);
    }

    if do_register && do_unregister {
        return Err(CliError::new(
            "参数错误: --register 与 --unregister 不能同时使用",
        ));
    }

    if (do_register || do_unregister) && (file_path.is_some() || config_path.is_some()) {
        return Err(CliError::new(
            "参数错误: --register / --unregister 不可与文件路径或 --config 混用",
        ));
    }

    if do_register {
        return Ok(CliAction::RegisterAssoc);
    }
    if do_unregister {
        return Ok(CliAction::UnregisterAssoc);
    }

    Ok(CliAction::OpenWindow {
        file_path,
        config_path,
    })
}

/// 生成标准命令行 `--help` 文本输出。
pub fn format_help_text() -> String {
    let extensions = SUPPORTED_EXTENSIONS.join(", ");
    format!(
        "{app} v{ver} — 纯粹、低内存的 Windows 桌面端 Markdown 阅读器\n\
         \n\
         用法:\n\
         \x20 MDReader.exe [选项] [<文件路径>]\n\
         \n\
         命令与选项:\n\
         \x20 <文件路径>          打开指定的 Markdown 文件并默认进入{default_mode}\n\
         \x20 --config <路径>     指定配置文件路径（默认查找 {cfg}）\n\
         \x20 --register          在 HKCU 注册 {exts} 文件打开方式\n\
         \x20 --unregister        从 HKCU 移除 MDReader 文件关联\n\
         \x20 -h, --help          显示本帮助信息并退出\n",
        app = APP_DISPLAY_NAME,
        ver = env!("CARGO_PKG_VERSION"),
        default_mode = AppMode::Reading.title_label(),
        cfg = CONFIG_FILE_NAME,
        exts = extensions,
    )
}

/// 检测句柄是否有效且指向真实输出对象（文件、管道或控制台）。
fn is_valid_std_handle(handle: HANDLE) -> bool {
    if handle.is_invalid() || handle == INVALID_HANDLE_VALUE || handle.0.is_null() {
        return false;
    }
    let file_type = unsafe { GetFileType(handle) };
    file_type != FILE_TYPE_UNKNOWN
}

/// 向指定标准句柄输出文本，在 GUI 子系统下同时兼容管道重定向与父控制台附加。
fn write_to_std_handle(std_id: STD_HANDLE, text: &str) {
    let mut handle = unsafe { GetStdHandle(std_id) }.unwrap_or(INVALID_HANDLE_VALUE);
    if !is_valid_std_handle(handle) {
        let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
        handle = unsafe { GetStdHandle(std_id) }.unwrap_or(INVALID_HANDLE_VALUE);
        if !is_valid_std_handle(handle) {
            return;
        }
    }

    let file_type = unsafe { GetFileType(handle) };
    if file_type == FILE_TYPE_CHAR {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut written = 0u32;
        if unsafe { WriteConsoleW(handle, &wide, Some(&mut written), None) }.is_ok() {
            return;
        }
    }

    let bytes = text.as_bytes();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let mut written = 0u32;
        let ok = unsafe { WriteFile(handle, Some(&bytes[offset..]), Some(&mut written), None) };
        if ok.is_err() || written == 0 {
            break;
        }
        offset += written as usize;
    }
}

/// 向标准输出写入 CLI 提示文本（兼容 GUI 子系统管道与父终端）。
pub fn write_cli_stdout(text: &str) {
    write_to_std_handle(STD_OUTPUT_HANDLE, text);
}

/// 向标准错误写入 CLI 错误文本（兼容 GUI 子系统管道与父终端）。
pub fn write_cli_stderr(text: &str) {
    write_to_std_handle(STD_ERROR_HANDLE, text);
}

#[cfg(test)]
mod tests {
    use super::{CliAction, format_help_text, parse_args};
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn test_parse_empty_opens_empty_window() {
        assert_eq!(
            parse_args(args(&[])).unwrap(),
            CliAction::OpenWindow {
                file_path: None,
                config_path: None,
            }
        );
    }

    #[test]
    fn test_parse_file_with_spaces_chinese_and_parens() {
        let p = r"C:\My Docs\笔记 (v1) [最终版]\readme.md";
        assert_eq!(
            parse_args(args(&[p])).unwrap(),
            CliAction::OpenWindow {
                file_path: Some(PathBuf::from(p)),
                config_path: None,
            }
        );
    }

    #[test]
    fn test_parse_config_space_and_equals_forms() {
        assert_eq!(
            parse_args(args(&["--config", r"C:\cfg\my config.json", "a.md"])).unwrap(),
            CliAction::OpenWindow {
                file_path: Some(PathBuf::from("a.md")),
                config_path: Some(PathBuf::from(r"C:\cfg\my config.json")),
            }
        );
        assert_eq!(
            parse_args(args(&["doc.md", r"--config=D:\便携 目录\config.json"])).unwrap(),
            CliAction::OpenWindow {
                file_path: Some(PathBuf::from("doc.md")),
                config_path: Some(PathBuf::from(r"D:\便携 目录\config.json")),
            }
        );
    }

    #[test]
    fn test_parse_double_dash_allows_leading_dash_filename() {
        assert_eq!(
            parse_args(args(&["--", "-weird-name.md"])).unwrap(),
            CliAction::OpenWindow {
                file_path: Some(PathBuf::from("-weird-name.md")),
                config_path: None,
            }
        );
    }

    #[test]
    fn test_parse_register_unregister_help_and_conflicts() {
        assert_eq!(parse_args(args(&["--register"])).unwrap(), CliAction::RegisterAssoc);
        assert_eq!(parse_args(args(&["--unregister"])).unwrap(), CliAction::UnregisterAssoc);
        assert_eq!(parse_args(args(&["--help"])).unwrap(), CliAction::ShowHelp);
        assert_eq!(parse_args(args(&["-h"])).unwrap(), CliAction::ShowHelp);

        assert!(parse_args(args(&["--register", "--unregister"])).is_err());
        assert!(parse_args(args(&["--register", "a.md"])).is_err());
        assert!(parse_args(args(&["--unregister", "--config", "c.json"])).is_err());
        assert!(parse_args(args(&["--config"])).is_err());
        assert!(parse_args(args(&["--config="])).is_err());
        assert!(parse_args(args(&["--unknown-flag"])).is_err());
        assert!(parse_args(args(&["a.md", "b.md"])).is_err());
    }

    #[test]
    fn test_help_text_contains_required_switches() {
        let help = format_help_text();
        assert!(help.contains("MDReader"));
        assert!(help.contains("--config"));
        assert!(help.contains("--register"));
        assert!(help.contains("--unregister"));
        assert!(help.contains("config.json"));
    }
}
