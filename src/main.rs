//! MDReader 应用程序主入口（Windows 原生 GUI 子系统）。
//!
//! - 启用 `#![windows_subsystem = "windows"]`，确保从资源管理器双击 `.md` 启动时零控制台黑框闪烁；
//! - `--help` / `--register` / `--unregister` 与非法参数错误通过 `platform::cli` 兼容管道重定向与父终端输出；
//! - GUI 模式启动前启用 Per-Monitor DPI V2 感知并初始化单线程 STA COM（供 WIC / 对话框 / UIA 使用）。

#![windows_subsystem = "windows"]

use std::process::ExitCode;

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

use mdreader::platform::assoc::{register_md_association, unregister_md_association};
use mdreader::platform::cli::{
    CliAction, format_help_text, parse_args, write_cli_stderr, write_cli_stdout,
};
use mdreader::ui::window::run_app_window;

/// STA COM RAII 守卫，确保退出前成对调用 `CoUninitialize`。
struct ComGuard(bool);

impl ComGuard {
    fn init_sta() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        Self(hr.is_ok())
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

fn main() -> ExitCode {
    let action = match parse_args(std::env::args_os().skip(1)) {
        Ok(act) => act,
        Err(err) => {
            write_cli_stderr(&format!("{err}\n"));
            return ExitCode::from(2);
        }
    };

    match action {
        CliAction::ShowHelp => {
            write_cli_stdout(&format_help_text());
            ExitCode::SUCCESS
        }
        CliAction::RegisterAssoc => {
            let exe = match std::env::current_exe() {
                Ok(p) => p,
                Err(e) => {
                    write_cli_stderr(&format!("无法获取当前程序路径: {e}\n"));
                    return ExitCode::FAILURE;
                }
            };
            match register_md_association(&exe) {
                Ok(()) => {
                    write_cli_stdout(&format!(
                        "已成功注册 .md / .markdown 打开方式 -> {}\n",
                        exe.display()
                    ));
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    write_cli_stderr(&format!("{e}\n"));
                    ExitCode::FAILURE
                }
            }
        }
        CliAction::UnregisterAssoc => match unregister_md_association() {
            Ok(()) => {
                write_cli_stdout("已从当前用户精准移除 MDReader 文件关联。\n");
                ExitCode::SUCCESS
            }
            Err(e) => {
                write_cli_stderr(&format!("{e}\n"));
                ExitCode::FAILURE
            }
        },
        CliAction::OpenWindow {
            file_path,
            config_path,
        } => {
            unsafe {
                let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            }
            let _com = ComGuard::init_sta();
            match run_app_window(file_path, config_path) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    write_cli_stderr(&format!("启动 MDReader 窗口失败: {e}\n"));
                    ExitCode::FAILURE
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use mdreader::platform::cli::format_help_text;

    #[test]
    fn test_help_text_contains_essential_contracts() {
        let help = format_help_text();
        assert!(help.contains("MDReader"));
        assert!(help.contains(".md"));
        assert!(help.contains("config.json"));
        assert!(help.contains("阅读模式"));
        assert!(help.contains("--register"));
    }
}
