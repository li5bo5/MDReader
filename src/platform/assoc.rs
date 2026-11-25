//! Windows `.md` / `.markdown` 文件关联管理模块（归属 Milestone 3）。
//!
//! 安全与无提权红线：
//! 1. 仅操作 `HKEY_CURRENT_USER`（HKCU），无需管理员权限即可完成“打开方式”与“默认应用”注册；
//! 2. 严禁读写受系统哈希保护的 `Explorer\FileExts\.md\UserChoice` 键；
//! 3. 所有 `shell\open\command` 模板均严格遵循 `"\"<AbsExePath>\" \"%1\""` 双引号规范；
//! 4. 卸载仅删除 MDReader 自建子树与自建值名，绝不删除 `.md` 键本身或其他软件的 ProgID；
//! 5. 注册与卸载完成后立即调用 `SHChangeNotify(SHCNE_ASSOCCHANGED, ...)` 刷新系统壳层缓存。

use std::error::Error;
use std::fmt;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, WIN32_ERROR};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, KEY_WRITE, REG_NONE, REG_OPTION_NON_VOLATILE, REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows::core::{PCWSTR, s, w};

use crate::core::SUPPORTED_EXTENSIONS;
use crate::platform::{APP_PROG_ID, REGISTERED_APP_NAME};

/// 注册表 `Capabilities` 应用描述文本。
const APP_DESCRIPTION: &str = "纯粹低内存 Windows Markdown 阅读器";

/// 注册表 `MDReader.Document` 友好文档类型名称。
const FRIENDLY_TYPE_NAME: &str = "Markdown Document";

/// 文件关联操作错误类型。
#[derive(Debug)]
pub enum AssocError {
    /// 传入的可执行文件路径无效（为空或非绝对路径无法定位）。
    InvalidExePath(String),
    /// Win32 注册表操作失败。
    Registry {
        operation: &'static str,
        subkey: String,
        code: u32,
    },
}

impl fmt::Display for AssocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidExePath(msg) => write!(f, "无效的可执行文件路径: {msg}"),
            Self::Registry {
                operation,
                subkey,
                code,
            } => write!(
                f,
                "注册表操作失败 ({operation}) [HKCU\\{subkey}]: Win32 错误码 {code}"
            ),
        }
    }
}

impl Error for AssocError {}

/// RAII 注册表句柄守卫，离开作用域自动调用 `RegCloseKey`。
struct RegKeyGuard(HKEY);

impl Drop for RegKeyGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            let _ = unsafe { RegCloseKey(self.0) };
        }
    }
}

/// 将字符串转为以 `\0` 结尾的 UTF-16 切片。
fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 规范化可执行文件绝对路径，并剥离 Windows `\\?\` 扩展路径前缀。
///
/// 若 `canonicalize` 成功则去除 `\\?\` 前缀（保证与资源管理器及普通绝对路径一致），
/// 失败时退回 `std::path::absolute` 或原路径。
pub fn normalize_exe_path(exe_path: &Path) -> Result<PathBuf, AssocError> {
    if exe_path.as_os_str().is_empty() {
        return Err(AssocError::InvalidExePath("路径为空".to_string()));
    }
    if exe_path.encode_wide_contains_nul() {
        return Err(AssocError::InvalidExePath("路径包含非法 NUL 字符".to_string()));
    }

    let resolved = if exe_path.is_absolute() {
        exe_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(exe_path))
            .unwrap_or_else(|_| exe_path.to_path_buf())
    };

    let text = resolved.to_string_lossy();
    if let Some(stripped) = text.strip_prefix(r"\\?\UNC\") {
        Ok(PathBuf::from(format!(r"\\{stripped}")))
    } else if let Some(stripped) = text.strip_prefix(r"\\?\") {
        Ok(PathBuf::from(stripped))
    } else {
        Ok(resolved)
    }
}

trait PathNulCheck {
    fn encode_wide_contains_nul(&self) -> bool;
}

impl PathNulCheck for Path {
    fn encode_wide_contains_nul(&self) -> bool {
        self.as_os_str().encode_wide().any(|u| u == 0)
    }
}

/// 构造 `shell\open\command` 模板：`"\"<AbsExePath>\" \"%1\""`。
pub fn build_open_command(abs_exe: &Path) -> String {
    format!("\"{}\" \"%1\"", abs_exe.to_string_lossy())
}

/// 构造 `DefaultIcon` 模板：`"\"<AbsExePath>\",0"`。
pub fn build_default_icon(abs_exe: &Path) -> String {
    format!("\"{}\",0", abs_exe.to_string_lossy())
}

/// 获取可执行文件名（默认回退 `"MDReader.exe"`）。
fn exe_file_name(abs_exe: &Path) -> String {
    abs_exe
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("MDReader.exe")
        .to_string()
}

/// 在指定根前缀下创建或打开 HKCU 子键并写入 `REG_SZ` 值（`value_name == ""` 表示 `(Default)`）。
fn set_hkcu_sz(subkey: &str, value_name: &str, data: &str) -> Result<(), AssocError> {
    let subkey_w = to_wide_null(subkey);
    let mut hkey = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
    };
    if status != WIN32_ERROR(0) {
        return Err(AssocError::Registry {
            operation: "RegCreateKeyExW",
            subkey: subkey.to_string(),
            code: status.0,
        });
    }
    let guard = RegKeyGuard(hkey);

    let name_w = if value_name.is_empty() {
        None
    } else {
        Some(to_wide_null(value_name))
    };
    let name_ptr = name_w
        .as_ref()
        .map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr()));
    let data_w = to_wide_null(data);
    let data_bytes = unsafe {
        std::slice::from_raw_parts(data_w.as_ptr() as *const u8, data_w.len() * std::mem::size_of::<u16>())
    };

    let set_status = unsafe { RegSetValueExW(guard.0, name_ptr, None, REG_SZ, Some(data_bytes)) };
    if set_status != WIN32_ERROR(0) {
        return Err(AssocError::Registry {
            operation: "RegSetValueExW(REG_SZ)",
            subkey: subkey.to_string(),
            code: set_status.0,
        });
    }
    Ok(())
}

/// 在 HKCU 子键下写入 0 字节 `REG_NONE` 标记值（用于 `OpenWithProgids` 标准约定）。
fn set_hkcu_none(subkey: &str, value_name: &str) -> Result<(), AssocError> {
    let subkey_w = to_wide_null(subkey);
    let mut hkey = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
    };
    if status != WIN32_ERROR(0) {
        return Err(AssocError::Registry {
            operation: "RegCreateKeyExW",
            subkey: subkey.to_string(),
            code: status.0,
        });
    }
    let guard = RegKeyGuard(hkey);

    let name_w = to_wide_null(value_name);
    let set_status =
        unsafe { RegSetValueExW(guard.0, PCWSTR(name_w.as_ptr()), None, REG_NONE, Some(&[])) };
    if set_status != WIN32_ERROR(0) {
        return Err(AssocError::Registry {
            operation: "RegSetValueExW(REG_NONE)",
            subkey: subkey.to_string(),
            code: set_status.0,
        });
    }
    Ok(())
}

/// 递归删除 HKCU 下指定子树（若键不存在则视为已清理并成功返回）。
fn delete_hkcu_tree_if_exists(subkey: &str) -> Result<(), AssocError> {
    let subkey_w = to_wide_null(subkey);
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(subkey_w.as_ptr())) };
    if status == WIN32_ERROR(0)
        || status == ERROR_FILE_NOT_FOUND
        || status == ERROR_PATH_NOT_FOUND
    {
        Ok(())
    } else {
        Err(AssocError::Registry {
            operation: "RegDeleteTreeW",
            subkey: subkey.to_string(),
            code: status.0,
        })
    }
}

/// 精准删除 HKCU 子键下的单个值名（键或值不存在均视为幂等成功，绝不删除同级第三方值）。
fn delete_hkcu_value_if_exists(subkey: &str, value_name: &str) -> Result<(), AssocError> {
    let subkey_w = to_wide_null(subkey);
    let mut hkey = HKEY::default();
    let open_status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            None,
            KEY_SET_VALUE,
            &mut hkey,
        )
    };
    if open_status == ERROR_FILE_NOT_FOUND || open_status == ERROR_PATH_NOT_FOUND {
        return Ok(());
    }
    if open_status != WIN32_ERROR(0) {
        return Err(AssocError::Registry {
            operation: "RegOpenKeyExW",
            subkey: subkey.to_string(),
            code: open_status.0,
        });
    }
    let guard = RegKeyGuard(hkey);

    let name_w = to_wide_null(value_name);
    let del_status = unsafe { RegDeleteValueW(guard.0, PCWSTR(name_w.as_ptr())) };
    if del_status == WIN32_ERROR(0)
        || del_status == ERROR_FILE_NOT_FOUND
        || del_status == ERROR_PATH_NOT_FOUND
    {
        Ok(())
    } else {
        Err(AssocError::Registry {
            operation: "RegDeleteValueW",
            subkey: subkey.to_string(),
            code: del_status.0,
        })
    }
}

/// 在指定的 HKCU 根前缀（正式为 `"Software"`，单元测试可传隔离沙盒前缀）下执行四组关联写入。
fn register_under_root(root_prefix: &str, exe_path: &Path) -> Result<(), AssocError> {
    let abs_exe = normalize_exe_path(exe_path)?;
    let open_cmd = build_open_command(&abs_exe);
    let default_icon = build_default_icon(&abs_exe);
    let exe_name = exe_file_name(&abs_exe);

    let classes_root = format!(r"{root_prefix}\Classes");
    let prog_id_key = format!(r"{classes_root}\{APP_PROG_ID}");
    let app_meta_key = format!(r"{classes_root}\Applications\{exe_name}");
    let cap_rel_path = format!(r"{root_prefix}\{REGISTERED_APP_NAME}\Capabilities");
    let reg_apps_key = format!(r"{root_prefix}\RegisteredApplications");

    // 1. ProgID 定义
    set_hkcu_sz(&prog_id_key, "", FRIENDLY_TYPE_NAME)?;
    set_hkcu_sz(&prog_id_key, "FriendlyTypeName", FRIENDLY_TYPE_NAME)?;
    set_hkcu_sz(&format!(r"{prog_id_key}\DefaultIcon"), "", &default_icon)?;
    set_hkcu_sz(&format!(r"{prog_id_key}\shell\open\command"), "", &open_cmd)?;

    // 2. 扩展名 OpenWithProgids 候选（.md / .markdown）
    for ext in SUPPORTED_EXTENSIONS {
        let owp_key = format!(r"{classes_root}\{ext}\OpenWithProgids");
        set_hkcu_none(&owp_key, APP_PROG_ID)?;
    }

    // 3. Applications 元数据（同时兼容真实可执行文件名与固定 MDReader.exe）
    set_hkcu_sz(&app_meta_key, "FriendlyAppName", REGISTERED_APP_NAME)?;
    set_hkcu_sz(&format!(r"{app_meta_key}\DefaultIcon"), "", &default_icon)?;
    set_hkcu_sz(
        &format!(r"{app_meta_key}\shell\open\command"),
        "",
        &open_cmd,
    )?;
    for ext in SUPPORTED_EXTENSIONS {
        set_hkcu_sz(&format!(r"{app_meta_key}\SupportedTypes"), ext, "")?;
    }

    // 4. 系统“默认应用” Capabilities 与 RegisteredApplications 注册
    set_hkcu_sz(&cap_rel_path, "ApplicationName", REGISTERED_APP_NAME)?;
    set_hkcu_sz(&cap_rel_path, "ApplicationDescription", APP_DESCRIPTION)?;
    let assoc_key = format!(r"{cap_rel_path}\FileAssociations");
    for ext in SUPPORTED_EXTENSIONS {
        set_hkcu_sz(&assoc_key, ext, APP_PROG_ID)?;
    }
    set_hkcu_sz(&reg_apps_key, REGISTERED_APP_NAME, &cap_rel_path)?;

    Ok(())
}

/// 在指定的 HKCU 根前缀下执行精准卸载清理（不误伤 `.md` 键或第三方项）。
fn unregister_under_root(root_prefix: &str, exe_name_hint: Option<&str>) -> Result<(), AssocError> {
    let classes_root = format!(r"{root_prefix}\Classes");

    // 1. 删除自建 ProgID 子树
    delete_hkcu_tree_if_exists(&format!(r"{classes_root}\{APP_PROG_ID}"))?;

    // 2. 精准移除 .md / .markdown 下的 MDReader.Document 值（保留键及第三方值）
    for ext in SUPPORTED_EXTENSIONS {
        let owp_key = format!(r"{classes_root}\{ext}\OpenWithProgids");
        delete_hkcu_value_if_exists(&owp_key, APP_PROG_ID)?;
    }

    // 3. 删除 Applications 元数据子树（包含固定 MDReader.exe 及当前进程文件名）
    delete_hkcu_tree_if_exists(&format!(r"{classes_root}\Applications\MDReader.exe"))?;
    if let Some(name) = exe_name_hint
        && !name.eq_ignore_ascii_case("MDReader.exe")
        && !name.is_empty()
    {
        delete_hkcu_tree_if_exists(&format!(r"{classes_root}\Applications\{name}"))?;
    }

    // 4. 删除 Capabilities 子树并移除 RegisteredApplications 中的 MDReader 值
    delete_hkcu_tree_if_exists(&format!(r"{root_prefix}\{REGISTERED_APP_NAME}"))?;
    delete_hkcu_value_if_exists(&format!(r"{root_prefix}\RegisteredApplications"), REGISTERED_APP_NAME)?;

    Ok(())
}

/// 通知 Windows 资源管理器文件关联已更新（按需懒加载 `shell32.dll`，避免阅读冷启动加载 Shell 子系统）。
fn notify_shell_assoc_changed() {
    type ShChangeNotifyFn = unsafe extern "system" fn(i32, u32, *const std::ffi::c_void, *const std::ffi::c_void);
    const SHCNE_ASSOCCHANGED_VAL: i32 = 0x0800_0000;
    const SHCNF_IDLIST_VAL: u32 = 0x0000;
    unsafe {
        if let Ok(hmod) = LoadLibraryW(w!("shell32.dll"))
            && let Some(proc) = GetProcAddress(hmod, s!("SHChangeNotify"))
        {
            let func: ShChangeNotifyFn = std::mem::transmute(proc);
            func(
                SHCNE_ASSOCCHANGED_VAL,
                SHCNF_IDLIST_VAL,
                std::ptr::null(),
                std::ptr::null(),
            );
        }
    }
}

/// 在 HKCU 注册 MDReader 为 `.md` / `.markdown` 打开方式与默认应用候选，并刷新 Shell。
pub fn register_md_association(exe_path: &Path) -> Result<(), AssocError> {
    register_under_root("Software", exe_path)?;
    notify_shell_assoc_changed();
    Ok(())
}

/// 从 HKCU 精准卸载 MDReader 文件关联（保留其他软件关联），并刷新 Shell。
pub fn unregister_md_association() -> Result<(), AssocError> {
    let current_exe_name = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    unregister_under_root("Software", current_exe_name.as_deref())?;
    notify_shell_assoc_changed();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        build_default_icon, build_open_command, delete_hkcu_tree_if_exists, normalize_exe_path,
        register_under_root, set_hkcu_sz, unregister_under_root,
    };
    use crate::platform::{APP_PROG_ID, REGISTERED_APP_NAME};
    use std::path::Path;
    use windows::Win32::Foundation::WIN32_ERROR;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, REG_VALUE_TYPE, RegCloseKey, RegOpenKeyExW,
        RegQueryValueExW,
    };
    use windows::core::PCWSTR;

    fn read_hkcu_sz(subkey: &str, value_name: &str) -> Option<String> {
        let subkey_w: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
        let mut hkey = HKEY::default();
        if unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey_w.as_ptr()),
                None,
                KEY_READ,
                &mut hkey,
            )
        } != WIN32_ERROR(0)
        {
            return None;
        }
        let name_w: Option<Vec<u16>> = if value_name.is_empty() {
            None
        } else {
            Some(value_name.encode_utf16().chain(std::iter::once(0)).collect())
        };
        let name_ptr = name_w
            .as_ref()
            .map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr()));
        let mut kind = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        let query_len = unsafe {
            RegQueryValueExW(hkey, name_ptr, None, Some(&mut kind), None, Some(&mut size))
        };
        if query_len != WIN32_ERROR(0) {
            let _ = unsafe { RegCloseKey(hkey) };
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        let query_val = unsafe {
            RegQueryValueExW(
                hkey,
                name_ptr,
                None,
                Some(&mut kind),
                Some(buf.as_mut_ptr()),
                Some(&mut size),
            )
        };
        let _ = unsafe { RegCloseKey(hkey) };
        if query_val != WIN32_ERROR(0) {
            return None;
        }
        let wide: Vec<u16> = buf
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let trimmed = wide
            .split(|&c| c == 0)
            .next()
            .unwrap_or(&[]);
        Some(String::from_utf16_lossy(trimmed))
    }

    fn value_exists(subkey: &str, value_name: &str) -> bool {
        let subkey_w: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
        let mut hkey = HKEY::default();
        if unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey_w.as_ptr()),
                None,
                KEY_READ,
                &mut hkey,
            )
        } != WIN32_ERROR(0)
        {
            return false;
        }
        let name_w: Vec<u16> = value_name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut size = 0u32;
        let st = unsafe {
            RegQueryValueExW(
                hkey,
                PCWSTR(name_w.as_ptr()),
                None,
                None,
                None,
                Some(&mut size),
            )
        };
        let _ = unsafe { RegCloseKey(hkey) };
        st == WIN32_ERROR(0)
    }

    #[test]
    fn test_command_and_icon_quoting() {
        let exe = Path::new(r"C:\Program Files\My App (测试)\MDReader.exe");
        assert_eq!(
            build_open_command(exe),
            r#""C:\Program Files\My App (测试)\MDReader.exe" "%1""#
        );
        assert_eq!(
            build_default_icon(exe),
            r#""C:\Program Files\My App (测试)\MDReader.exe",0"#
        );
    }

    #[test]
    fn test_normalize_strips_verbatim_prefix() {
        let p = normalize_exe_path(Path::new(r"\\?\C:\Tools\MDReader.exe")).unwrap();
        assert_eq!(p, Path::new(r"C:\Tools\MDReader.exe"));
        assert!(normalize_exe_path(Path::new("")).is_err());
    }

    #[test]
    fn test_isolated_hkcu_sandbox_register_and_precise_unregister() {
        let sandbox = format!(r"Software\MDReader_UnitTest_{}", std::process::id());
        let _ = delete_hkcu_tree_if_exists(&sandbox);

        // 预埋第三方 OpenWithProgids 与 RegisteredApplications 哨兵值，验证卸载绝不误伤
        let md_owp = format!(r"{sandbox}\Classes\.md\OpenWithProgids");
        let reg_apps = format!(r"{sandbox}\RegisteredApplications");
        set_hkcu_sz(&md_owp, "ThirdParty.Markdown", "").unwrap();
        set_hkcu_sz(&reg_apps, "ThirdPartyApp", r"Software\ThirdParty\Capabilities").unwrap();

        let fake_exe = Path::new(r"C:\Sand Box\MDReader.exe");
        register_under_root(&sandbox, fake_exe).unwrap();

        assert_eq!(
            read_hkcu_sz(&format!(r"{sandbox}\Classes\{APP_PROG_ID}\shell\open\command"), ""),
            Some(r#""C:\Sand Box\MDReader.exe" "%1""#.to_string())
        );
        assert!(value_exists(&md_owp, APP_PROG_ID));
        assert!(value_exists(
            &format!(r"{sandbox}\Classes\.markdown\OpenWithProgids"),
            APP_PROG_ID
        ));
        assert_eq!(
            read_hkcu_sz(&reg_apps, REGISTERED_APP_NAME),
            Some(format!(r"{sandbox}\{REGISTERED_APP_NAME}\Capabilities"))
        );

        unregister_under_root(&sandbox, Some("MDReader.exe")).unwrap();
        // 自建键值全部清除
        assert!(read_hkcu_sz(&format!(r"{sandbox}\Classes\{APP_PROG_ID}"), "").is_none());
        assert!(!value_exists(&md_owp, APP_PROG_ID));
        assert!(!value_exists(&reg_apps, REGISTERED_APP_NAME));
        // 第三方哨兵完整保留
        assert!(value_exists(&md_owp, "ThirdParty.Markdown"));
        assert_eq!(
            read_hkcu_sz(&reg_apps, "ThirdPartyApp"),
            Some(r"Software\ThirdParty\Capabilities".to_string())
        );

        // 幂等再次卸载不报错
        unregister_under_root(&sandbox, Some("MDReader.exe")).unwrap();

        let _ = delete_hkcu_tree_if_exists(&sandbox);
    }
}
