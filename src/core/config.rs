//! 单一本地 JSON 配置文件管理、3 层路径查找、容错校验与安全几何记忆模块。
//!
//! 严格遵循极简设置哲学：界面不暴露复杂设置面板，全部偏好收敛于 `config.json`。
//! 解析时先剥离 UTF-8 BOM 并反序列化为 `serde_json::Value`，再逐字段做精确边界校验，
//! 单个字段类型错误或越界仅回退该字段默认值，整文件损坏时不阻塞启动且拒绝覆写原配置。

use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::core::CONFIG_FILE_NAME;
use crate::core::storage::{self, StorageError};

/// 系统 `%APPDATA%` 下的配置子目录名称。
pub const APP_CONFIG_DIR_NAME: &str = "MDReader";

/// 主题外观模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThemeMode {
    /// 柔和暖白亮色阅读主题（默认）。
    #[default]
    Light,
    /// 深灰蓝低眩光暗色阅读主题。
    Dark,
    /// 跟随 Windows 系统应用主题。
    System,
}

/// 自定义调色板覆盖配置（均须为 `#RRGGBB` 6 位十六进制颜色串）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CustomColors {
    /// 画布背景色。
    pub background: Option<String>,
    /// 正文文字颜色。
    pub text: Option<String>,
    /// 行内与围栏代码块背景色。
    #[serde(alias = "code_background")]
    pub code_background: Option<String>,
    /// 引用块背景色。
    #[serde(alias = "quote_background")]
    pub quote_background: Option<String>,
    /// 超链接颜色。
    pub link: Option<String>,
    /// 分割线、表格与代码块边框颜色。
    pub border: Option<String>,
}

/// MDReader 完整应用配置结构体（16 项核心字段 + 可选 `colors`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    /// 主题模式（`light` | `dark` | `system`）。
    pub theme: ThemeMode,
    /// 正文无衬线字体族回退链。
    #[serde(alias = "font_family")]
    pub font_family: String,
    /// 等宽代码字体族回退链。
    #[serde(alias = "mono_font_family")]
    pub mono_font_family: String,
    /// 阅读区正文字号（范围 `12.0..=32.0`）。
    #[serde(alias = "font_size")]
    pub font_size: f32,
    /// 阅读区行高倍数（范围 `1.2..=2.5`）。
    #[serde(alias = "line_height")]
    pub line_height: f32,
    /// 阅读区版心最大像素宽度（范围 `480..=1400`）。
    #[serde(alias = "max_width")]
    pub max_width: u32,
    /// 阅读区页边距像素（范围 `12..=120`）。
    #[serde(alias = "page_padding")]
    pub page_padding: u32,
    /// 修改模式编辑区字号（范围 `12.0..=28.0`）。
    #[serde(alias = "editor_font_size")]
    pub editor_font_size: f32,
    /// 修改模式编辑区行高倍数（范围 `1.2..=2.5`）。
    #[serde(alias = "editor_line_height")]
    pub editor_line_height: f32,
    /// 修改模式 Tab 缩进空格宽度（范围 `2..=8`）。
    #[serde(alias = "editor_tab_size")]
    pub editor_tab_size: u8,
    /// 修改模式是否显示行号。
    #[serde(alias = "show_line_numbers")]
    pub show_line_numbers: bool,
    /// 修改模式是否自动折行。
    #[serde(alias = "word_wrap")]
    pub word_wrap: bool,
    /// 关闭窗口时是否记忆窗口宽高。
    #[serde(alias = "remember_window_geometry")]
    pub remember_window_geometry: bool,
    /// 初始/记忆窗口宽度（范围 `400..=3840`）。
    #[serde(alias = "window_width")]
    pub window_width: u32,
    /// 初始/记忆窗口高度（范围 `300..=2160`）。
    #[serde(alias = "window_height")]
    pub window_height: u32,
    /// 是否监听配置文件或当前文档变更。
    #[serde(alias = "watch_file_changes")]
    pub watch_file_changes: bool,
    /// 可选自定义颜色覆盖。
    pub colors: Option<CustomColors>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Light,
            font_family: "Microsoft YaHei UI, Segoe UI, sans-serif".to_string(),
            mono_font_family: "Cascadia Code, Consolas, monospace".to_string(),
            font_size: 16.0,
            line_height: 1.65,
            max_width: 760,
            page_padding: 40,
            editor_font_size: 15.0,
            editor_line_height: 1.6,
            editor_tab_size: 4,
            show_line_numbers: true,
            word_wrap: true,
            remember_window_geometry: true,
            window_width: 960,
            window_height: 760,
            watch_file_changes: false,
            colors: None,
        }
    }
}

/// 配置加载结果与诊断状态。
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigLoadResult {
    /// 最终生效的配置对象（保证全部字段处于合法范围）。
    pub config: AppConfig,
    /// 实际读取到的配置文件路径；若文件不存在或未提供路径则为 `None`。
    pub source_path: Option<PathBuf>,
    /// 非阻塞诊断警告（文件缺失、损坏或局部字段回退说明）。
    pub warning: Option<String>,
    /// 标识源文件是否因 I/O 错误、空文件、语法损坏或根节点非 JSON 对象而整文件失效。
    pub source_is_corrupt: bool,
}

/// 窗口几何写回配置文件时的错误类型。
#[derive(Debug)]
pub enum ConfigSaveError {
    /// 源文件处于损坏或无法安全解析状态，拒绝覆写以保护用户数据。
    CorruptConfigProtected,
    /// 传入的窗口宽高不在合法范围内（如最小化 `0x0`）。
    InvalidGeometry { width: u32, height: u32 },
    /// 底层原子写入失败（如只读权限或路径错误）。
    Storage(StorageError),
    /// JSON 序列化失败。
    Serialize(String),
}

impl fmt::Display for ConfigSaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CorruptConfigProtected => {
                write!(f, "配置文件处于损坏或不可读状态，已禁止自动覆写保护原文件")
            }
            Self::InvalidGeometry { width, height } => {
                write!(f, "窗口尺寸 ({width}x{height}) 超出有效范围，跳过写回")
            }
            Self::Storage(err) => write!(f, "保存配置文件失败: {err}"),
            Self::Serialize(msg) => write!(f, "序列化配置失败: {msg}"),
        }
    }
}

impl Error for ConfigSaveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(err) => Some(err),
            Self::CorruptConfigProtected
            | Self::InvalidGeometry { .. }
            | Self::Serialize(_) => None,
        }
    }
}

impl From<StorageError> for ConfigSaveError {
    fn from(err: StorageError) -> Self {
        Self::Storage(err)
    }
}

/// 配置文件元数据快照，用于无后台线程的轻量变更探测。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
    /// 文件最后修改时间。
    pub modified: Option<SystemTime>,
    /// 文件字节大小。
    pub len: u64,
}

/// 轻量级配置文件变更监听器（供主窗口热重载与激活防抖调用）。
#[derive(Debug, Clone)]
pub struct ConfigWatcher {
    path: Option<PathBuf>,
    last_stamp: Option<FileStamp>,
}

impl ConfigWatcher {
    /// 创建监听器并记录目标路径初始文件快照。
    pub fn new(path: Option<PathBuf>) -> Self {
        let last_stamp = path.as_deref().and_then(Self::probe_stamp);
        Self { path, last_stamp }
    }

    /// 探测指定路径的当前文件快照；文件不存在或不可访问时返回 `None`。
    pub fn probe_stamp(path: &Path) -> Option<FileStamp> {
        let meta = fs::metadata(path).ok()?;
        if !meta.is_file() {
            return None;
        }
        Some(FileStamp {
            modified: meta.modified().ok(),
            len: meta.len(),
        })
    }

    /// 检测自上次探测以来配置文件是否发生新建、修改或删除；如有变化同步更新内部快照。
    pub fn poll_changed(&mut self) -> bool {
        let current = self.path.as_deref().and_then(Self::probe_stamp);
        if current != self.last_stamp {
            self.last_stamp = current;
            true
        } else {
            false
        }
    }

    /// 仅当配置文件发生变化时重新加载并返回最新结果。
    pub fn reload_if_changed(&mut self) -> Option<ConfigLoadResult> {
        if self.poll_changed() {
            Some(load_config(self.path.as_deref()))
        } else {
            None
        }
    }

    /// 刷新快照并强制执行一次完整加载（适用于 `Ctrl+Shift+R` / `F5` 手动热重载）。
    pub fn force_reload(&mut self) -> ConfigLoadResult {
        self.last_stamp = self.path.as_deref().and_then(Self::probe_stamp);
        load_config(self.path.as_deref())
    }

    /// 返回当前监听的配置文件路径。
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

/// 纯函数：按三层优先级解析配置文件路径。
///
/// 优先级顺序：
/// 1. `cli_override` 显式路径（即使尚不存在也直接返回）；
/// 2. `exe_dir` 同级目录 `./config.json`（仅当其为普通文件时命中，防止同名目录误判）；
/// 3. `appdata` 下的 `MDReader\config.json`；
/// 4. 若 `appdata` 缺失或为空，兜底返回 `exe_dir\config.json`。
pub fn resolve_config_path_with_appdata(
    cli_override: Option<&Path>,
    exe_dir: &Path,
    appdata: Option<&Path>,
) -> PathBuf {
    if let Some(override_path) = cli_override {
        return override_path.to_path_buf();
    }

    let portable = exe_dir.join(CONFIG_FILE_NAME);
    if portable.is_file() {
        return portable;
    }

    if let Some(appdata_dir) = appdata
        && !appdata_dir.as_os_str().is_empty()
    {
        return appdata_dir
            .join(APP_CONFIG_DIR_NAME)
            .join(CONFIG_FILE_NAME);
    }

    portable
}

/// 按三层优先级解析配置文件路径（自动读取环境变量 `APPDATA`）。
pub fn resolve_config_path(cli_override: Option<&Path>, exe_dir: &Path) -> PathBuf {
    let appdata = env::var_os("APPDATA")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    resolve_config_path_with_appdata(cli_override, exe_dir, appdata.as_deref())
}

/// 校验字符串是否为合法的 `#RRGGBB` 6 位十六进制颜色字面量。
pub fn is_valid_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// 加载指定路径的配置文件（内部等价于 `load_config_with_source(path, false)`）。
pub fn load_config(path: Option<&Path>) -> ConfigLoadResult {
    load_config_with_source(path, false)
}

/// 加载指定路径的配置文件，可显式标注是否为用户通过 `--config` 传入的路径。
pub fn load_config_with_source(
    path: Option<&Path>,
    is_explicit_cli_path: bool,
) -> ConfigLoadResult {
    let Some(target) = path else {
        return ConfigLoadResult {
            config: AppConfig::default(),
            source_path: None,
            warning: None,
            source_is_corrupt: false,
        };
    };

    if target.as_os_str().is_empty() {
        return ConfigLoadResult {
            config: AppConfig::default(),
            source_path: None,
            warning: Some("配置文件路径为空，已回退默认配置".to_string()),
            source_is_corrupt: false,
        };
    }

    let raw = match fs::read_to_string(target) {
        Ok(content) => content,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            let warning = if is_explicit_cli_path {
                Some(format!(
                    "指定的配置文件不存在，已使用默认配置: {}",
                    target.display()
                ))
            } else {
                None
            };
            return ConfigLoadResult {
                config: AppConfig::default(),
                source_path: None,
                warning,
                source_is_corrupt: false,
            };
        }
        Err(err) => {
            return ConfigLoadResult {
                config: AppConfig::default(),
                source_path: Some(target.to_path_buf()),
                warning: Some(format!(
                    "无法读取配置文件（{}），已回退默认配置: {err}",
                    target.display()
                )),
                source_is_corrupt: true,
            };
        }
    };

    let text = raw.strip_prefix('\u{FEFF}').unwrap_or(&raw);
    if text.trim().is_empty() {
        return ConfigLoadResult {
            config: AppConfig::default(),
            source_path: Some(target.to_path_buf()),
            warning: Some(format!(
                "配置文件为空（{}），已回退默认配置",
                target.display()
            )),
            source_is_corrupt: true,
        };
    }

    let parsed = match serde_json::from_str::<Value>(text) {
        Ok(val) => val,
        Err(err) => {
            return ConfigLoadResult {
                config: AppConfig::default(),
                source_path: Some(target.to_path_buf()),
                warning: Some(format!(
                    "配置文件 JSON 语法错误（{}），已回退默认配置: {err}",
                    target.display()
                )),
                source_is_corrupt: true,
            };
        }
    };

    let Some(map) = parsed.as_object() else {
        return ConfigLoadResult {
            config: AppConfig::default(),
            source_path: Some(target.to_path_buf()),
            warning: Some(format!(
                "配置文件根节点必须为 JSON 对象（{}），已回退默认配置",
                target.display()
            )),
            source_is_corrupt: true,
        };
    };

    let (config, warnings) = parse_config_object(map);
    let warning = if warnings.is_empty() {
        None
    } else {
        Some(warnings.join("; "))
    };

    ConfigLoadResult {
        config,
        source_path: Some(target.to_path_buf()),
        warning,
        source_is_corrupt: false,
    }
}

/// 在对象中优先查找 `camelCase` 键，不存在时回退查找 `snake_case` 键。
fn lookup_field<'a>(
    map: &'a Map<String, Value>,
    camel: &str,
    snake: Option<&str>,
) -> Option<(&'a str, &'a Value)> {
    if let Some((k, v)) = map.get_key_value(camel) {
        return Some((k.as_str(), v));
    }
    if let Some(snake_key) = snake
        && let Some((k, v)) = map.get_key_value(snake_key)
    {
        return Some((k.as_str(), v));
    }
    None
}

/// 逐字段解析并验证 JSON 对象内容，坏字段单独回退默认值。
fn parse_config_object(map: &Map<String, Value>) -> (AppConfig, Vec<String>) {
    let defaults = AppConfig::default();
    let mut cfg = AppConfig::default();
    let mut warnings = Vec::new();

    cfg.theme = parse_theme_field(map, defaults.theme, &mut warnings);
    cfg.font_family = parse_string_field(
        map,
        "fontFamily",
        Some("font_family"),
        defaults.font_family,
        &mut warnings,
    );
    cfg.mono_font_family = parse_string_field(
        map,
        "monoFontFamily",
        Some("mono_font_family"),
        defaults.mono_font_family,
        &mut warnings,
    );
    cfg.font_size = parse_f32_field(
        map,
        "fontSize",
        Some("font_size"),
        12.0,
        32.0,
        defaults.font_size,
        &mut warnings,
    );
    cfg.line_height = parse_f32_field(
        map,
        "lineHeight",
        Some("line_height"),
        1.2,
        2.5,
        defaults.line_height,
        &mut warnings,
    );
    cfg.max_width = parse_u32_field(
        map,
        "maxWidth",
        Some("max_width"),
        480,
        1400,
        defaults.max_width,
        &mut warnings,
    );
    cfg.page_padding = parse_u32_field(
        map,
        "pagePadding",
        Some("page_padding"),
        12,
        120,
        defaults.page_padding,
        &mut warnings,
    );
    cfg.editor_font_size = parse_f32_field(
        map,
        "editorFontSize",
        Some("editor_font_size"),
        12.0,
        28.0,
        defaults.editor_font_size,
        &mut warnings,
    );
    cfg.editor_line_height = parse_f32_field(
        map,
        "editorLineHeight",
        Some("editor_line_height"),
        1.2,
        2.5,
        defaults.editor_line_height,
        &mut warnings,
    );
    cfg.editor_tab_size = parse_u8_field(
        map,
        "editorTabSize",
        Some("editor_tab_size"),
        2,
        8,
        defaults.editor_tab_size,
        &mut warnings,
    );
    cfg.show_line_numbers = parse_bool_field(
        map,
        "showLineNumbers",
        Some("show_line_numbers"),
        defaults.show_line_numbers,
        &mut warnings,
    );
    cfg.word_wrap = parse_bool_field(
        map,
        "wordWrap",
        Some("word_wrap"),
        defaults.word_wrap,
        &mut warnings,
    );
    cfg.remember_window_geometry = parse_bool_field(
        map,
        "rememberWindowGeometry",
        Some("remember_window_geometry"),
        defaults.remember_window_geometry,
        &mut warnings,
    );
    cfg.window_width = parse_u32_field(
        map,
        "windowWidth",
        Some("window_width"),
        400,
        3840,
        defaults.window_width,
        &mut warnings,
    );
    cfg.window_height = parse_u32_field(
        map,
        "windowHeight",
        Some("window_height"),
        300,
        2160,
        defaults.window_height,
        &mut warnings,
    );
    cfg.watch_file_changes = parse_bool_field(
        map,
        "watchFileChanges",
        Some("watch_file_changes"),
        defaults.watch_file_changes,
        &mut warnings,
    );
    cfg.colors = parse_colors_field(map, &mut warnings);

    (cfg, warnings)
}

fn parse_theme_field(
    map: &Map<String, Value>,
    default: ThemeMode,
    warnings: &mut Vec<String>,
) -> ThemeMode {
    let Some((key, value)) = lookup_field(map, "theme", None) else {
        return default;
    };
    let Some(raw) = value.as_str() else {
        warnings.push(format!("字段 `{key}` 必须为字符串，已回退默认主题"));
        return default;
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "light" => ThemeMode::Light,
        "dark" => ThemeMode::Dark,
        "system" => ThemeMode::System,
        _ => {
            warnings.push(format!(
                "字段 `{key}` 值 `{raw}` 无效（仅支持 light/dark/system），已回退默认主题"
            ));
            default
        }
    }
}

fn parse_string_field(
    map: &Map<String, Value>,
    camel: &str,
    snake: Option<&str>,
    default: String,
    warnings: &mut Vec<String>,
) -> String {
    let Some((key, value)) = lookup_field(map, camel, snake) else {
        return default;
    };
    let Some(raw) = value.as_str() else {
        warnings.push(format!("字段 `{key}` 必须为非空字符串，已回退默认值"));
        return default;
    };
    let trimmed = raw.trim();
    let char_len = trimmed.chars().count();
    if char_len == 0 || char_len > 256 {
        warnings.push(format!(
            "字段 `{key}` trim 后长度必须位于 1..=256，已回退默认值"
        ));
        return default;
    }
    trimmed.to_string()
}

fn parse_f32_field(
    map: &Map<String, Value>,
    camel: &str,
    snake: Option<&str>,
    min: f64,
    max: f64,
    default: f32,
    warnings: &mut Vec<String>,
) -> f32 {
    let Some((key, value)) = lookup_field(map, camel, snake) else {
        return default;
    };
    let Some(num) = value.as_f64() else {
        warnings.push(format!(
            "字段 `{key}` 必须为有限数字（范围 {min}..={max}），已回退默认值 {default}"
        ));
        return default;
    };
    if !num.is_finite() || num < min || num > max {
        warnings.push(format!(
            "字段 `{key}` 数值 {num} 超出允许范围 [{min}, {max}]，已回退默认值 {default}"
        ));
        return default;
    }
    let out = num as f32;
    if !out.is_finite() {
        warnings.push(format!(
            "字段 `{key}` 数值溢出 f32 范围，已回退默认值 {default}"
        ));
        return default;
    }
    out.clamp(min as f32, max as f32)
}

fn parse_u32_field(
    map: &Map<String, Value>,
    camel: &str,
    snake: Option<&str>,
    min: u32,
    max: u32,
    default: u32,
    warnings: &mut Vec<String>,
) -> u32 {
    let Some((key, value)) = lookup_field(map, camel, snake) else {
        return default;
    };
    let Some(num) = value.as_u64() else {
        warnings.push(format!(
            "字段 `{key}` 必须为整数（范围 {min}..={max}），已回退默认值 {default}"
        ));
        return default;
    };
    match u32::try_from(num) {
        Ok(v) if (min..=max).contains(&v) => v,
        _ => {
            warnings.push(format!(
                "字段 `{key}` 数值 {num} 超出允许范围 [{min}, {max}]，已回退默认值 {default}"
            ));
            default
        }
    }
}

fn parse_u8_field(
    map: &Map<String, Value>,
    camel: &str,
    snake: Option<&str>,
    min: u8,
    max: u8,
    default: u8,
    warnings: &mut Vec<String>,
) -> u8 {
    let Some((key, value)) = lookup_field(map, camel, snake) else {
        return default;
    };
    let Some(num) = value.as_u64() else {
        warnings.push(format!(
            "字段 `{key}` 必须为整数（范围 {min}..={max}），已回退默认值 {default}"
        ));
        return default;
    };
    match u8::try_from(num) {
        Ok(v) if (min..=max).contains(&v) => v,
        _ => {
            warnings.push(format!(
                "字段 `{key}` 数值 {num} 超出允许范围 [{min}, {max}]，已回退默认值 {default}"
            ));
            default
        }
    }
}

fn parse_bool_field(
    map: &Map<String, Value>,
    camel: &str,
    snake: Option<&str>,
    default: bool,
    warnings: &mut Vec<String>,
) -> bool {
    let Some((key, value)) = lookup_field(map, camel, snake) else {
        return default;
    };
    match value.as_bool() {
        Some(v) => v,
        None => {
            warnings.push(format!("字段 `{key}` 必须为布尔值，已回退默认值 {default}"));
            default
        }
    }
}

fn parse_color_slot(
    colors_map: &Map<String, Value>,
    camel: &str,
    snake: Option<&str>,
    warnings: &mut Vec<String>,
) -> Option<String> {
    let (_, value) = lookup_field(colors_map, camel, snake)?;
    if value.is_null() {
        return None;
    }
    let Some(raw) = value.as_str() else {
        warnings.push(format!("自定义颜色 `colors.{camel}` 必须为 #RRGGBB 字符串，已忽略"));
        return None;
    };
    let trimmed = raw.trim();
    if is_valid_hex_color(trimmed) {
        Some(trimmed.to_string())
    } else {
        warnings.push(format!(
            "自定义颜色 `colors.{camel}` 格式 `{raw}` 无效（须为 #RRGGBB），已忽略"
        ));
        None
    }
}

fn parse_colors_field(
    map: &Map<String, Value>,
    warnings: &mut Vec<String>,
) -> Option<CustomColors> {
    let (_, value) = lookup_field(map, "colors", None)?;
    if value.is_null() {
        return None;
    }
    let Some(colors_map) = value.as_object() else {
        warnings.push("字段 `colors` 必须为 JSON 对象，已回退默认配色".to_string());
        return None;
    };

    let custom = CustomColors {
        background: parse_color_slot(colors_map, "background", None, warnings),
        text: parse_color_slot(colors_map, "text", None, warnings),
        code_background: parse_color_slot(
            colors_map,
            "codeBackground",
            Some("code_background"),
            warnings,
        ),
        quote_background: parse_color_slot(
            colors_map,
            "quoteBackground",
            Some("quote_background"),
            warnings,
        ),
        link: parse_color_slot(colors_map, "link", None, warnings),
        border: parse_color_slot(colors_map, "border", None, warnings),
    };

    if custom.background.is_none()
        && custom.text.is_none()
        && custom.code_background.is_none()
        && custom.quote_background.is_none()
        && custom.link.is_none()
        && custom.border.is_none()
    {
        None
    } else {
        Some(custom)
    }
}

/// 在确保安全的前提下，将窗口最新宽高原子更新回配置文件。
///
/// 返回 `Ok(true)` 表示已实际写盘；`Ok(false)` 表示配置禁用了几何记忆。
/// 若初始加载即损坏、或运行期磁盘配置已被改坏/清空、或目标为只读，则拒绝覆写并返回错误。
pub fn save_window_geometry_if_valid(
    load_result: &ConfigLoadResult,
    target_path: &Path,
    width: u32,
    height: u32,
) -> Result<bool, ConfigSaveError> {
    if load_result.source_is_corrupt {
        return Err(ConfigSaveError::CorruptConfigProtected);
    }
    if !load_result.config.remember_window_geometry {
        return Ok(false);
    }
    if !(400..=3840).contains(&width) || !(300..=2160).contains(&height) {
        return Err(ConfigSaveError::InvalidGeometry { width, height });
    }
    if target_path.as_os_str().is_empty() {
        return Err(ConfigSaveError::Storage(StorageError::InvalidPath(
            "目标配置文件路径不能为空".to_string(),
        )));
    }

    let next_value = match fs::metadata(target_path) {
        Ok(meta) => {
            if meta.is_dir() {
                return Err(ConfigSaveError::CorruptConfigProtected);
            }
            // 二次防线：重读磁盘上的当前文件，防止运行期被外部改坏后被几何保存抹除
            let raw = fs::read_to_string(target_path)
                .map_err(|_| ConfigSaveError::CorruptConfigProtected)?;
            let text = raw.strip_prefix('\u{FEFF}').unwrap_or(&raw);
            if text.trim().is_empty() {
                return Err(ConfigSaveError::CorruptConfigProtected);
            }
            let parsed = serde_json::from_str::<Value>(text)
                .map_err(|_| ConfigSaveError::CorruptConfigProtected)?;
            let Value::Object(mut obj) = parsed else {
                return Err(ConfigSaveError::CorruptConfigProtected);
            };
            obj.insert("windowWidth".to_string(), Value::from(width));
            obj.insert("windowHeight".to_string(), Value::from(height));
            obj.remove("window_width");
            obj.remove("window_height");
            Value::Object(obj)
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {
            if let Some(parent) = target_path.parent()
                && !parent.as_os_str().is_empty()
            {
                fs::create_dir_all(parent).map_err(StorageError::Io)?;
            }
            let mut cfg = load_result.config.clone();
            cfg.window_width = width;
            cfg.window_height = height;
            serde_json::to_value(&cfg)
                .map_err(|e| ConfigSaveError::Serialize(e.to_string()))?
        }
        Err(err) => return Err(ConfigSaveError::Storage(StorageError::Io(err))),
    };

    let mut serialized = serde_json::to_string_pretty(&next_value)
        .map_err(|e| ConfigSaveError::Serialize(e.to_string()))?;
    serialized.push('\n');

    storage::write_bytes_atomic(target_path, serialized.as_bytes())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR_SEQ: AtomicU64 = AtomicU64::new(0);

    struct TestTempDir {
        path: PathBuf,
    }

    impl TestTempDir {
        fn new(label: &str) -> Self {
            let seq = TEST_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
            let pid = std::process::id();
            let dir = std::env::temp_dir().join(format!("mdreader_config_test_{label}_{pid}_{seq}"));
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
    fn test_resolve_config_path_cli_overrides_portable_and_appdata() {
        let dir = TestTempDir::new("cli_override");
        let exe_dir = dir.path.join("exe");
        let appdata = dir.path.join("appdata");
        fs::create_dir_all(&exe_dir).unwrap();
        fs::create_dir_all(appdata.join(APP_CONFIG_DIR_NAME)).unwrap();
        fs::write(exe_dir.join(CONFIG_FILE_NAME), "{}").unwrap();
        fs::write(appdata.join(APP_CONFIG_DIR_NAME).join(CONFIG_FILE_NAME), "{}").unwrap();

        let explicit = dir.path.join("custom_nonexistent.json");
        let resolved = resolve_config_path_with_appdata(
            Some(&explicit),
            &exe_dir,
            Some(&appdata),
        );
        assert_eq!(resolved, explicit);
    }

    #[test]
    fn test_resolve_config_path_portable_overrides_appdata() {
        let dir = TestTempDir::new("portable_override");
        let exe_dir = dir.path.join("exe");
        let appdata = dir.path.join("appdata");
        fs::create_dir_all(&exe_dir).unwrap();
        fs::create_dir_all(appdata.join(APP_CONFIG_DIR_NAME)).unwrap();

        let portable = exe_dir.join(CONFIG_FILE_NAME);
        fs::write(&portable, "{}").unwrap();
        fs::write(appdata.join(APP_CONFIG_DIR_NAME).join(CONFIG_FILE_NAME), "{}").unwrap();

        let resolved = resolve_config_path_with_appdata(None, &exe_dir, Some(&appdata));
        assert_eq!(resolved, portable);
    }

    #[test]
    fn test_resolve_config_path_ignores_directory_named_config_json() {
        let dir = TestTempDir::new("dir_named_config");
        let exe_dir = dir.path.join("exe");
        let appdata = dir.path.join("appdata");
        fs::create_dir_all(exe_dir.join(CONFIG_FILE_NAME)).unwrap();
        fs::create_dir_all(&appdata).unwrap();

        let resolved = resolve_config_path_with_appdata(None, &exe_dir, Some(&appdata));
        assert_eq!(
            resolved,
            appdata.join(APP_CONFIG_DIR_NAME).join(CONFIG_FILE_NAME)
        );
    }

    #[test]
    fn test_resolve_config_path_falls_back_to_appdata_then_exe_dir() {
        let dir = TestTempDir::new("fallback");
        let exe_dir = dir.path.join("exe");
        let appdata = dir.path.join("appdata");
        fs::create_dir_all(&exe_dir).unwrap();

        assert_eq!(
            resolve_config_path_with_appdata(None, &exe_dir, Some(&appdata)),
            appdata.join(APP_CONFIG_DIR_NAME).join(CONFIG_FILE_NAME)
        );
        assert_eq!(
            resolve_config_path_with_appdata(None, &exe_dir, None),
            exe_dir.join(CONFIG_FILE_NAME)
        );
        assert_eq!(
            resolve_config_path_with_appdata(None, &exe_dir, Some(Path::new(""))),
            exe_dir.join(CONFIG_FILE_NAME)
        );
    }

    #[test]
    fn test_default_config_values_match_spec() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.theme, ThemeMode::Light);
        assert_eq!(cfg.font_family, "Microsoft YaHei UI, Segoe UI, sans-serif");
        assert_eq!(cfg.mono_font_family, "Cascadia Code, Consolas, monospace");
        assert_eq!(cfg.font_size, 16.0);
        assert_eq!(cfg.line_height, 1.65);
        assert_eq!(cfg.max_width, 760);
        assert_eq!(cfg.page_padding, 40);
        assert_eq!(cfg.editor_font_size, 15.0);
        assert_eq!(cfg.editor_line_height, 1.6);
        assert_eq!(cfg.editor_tab_size, 4);
        assert!(cfg.show_line_numbers);
        assert!(cfg.word_wrap);
        assert!(cfg.remember_window_geometry);
        assert_eq!(cfg.window_width, 960);
        assert_eq!(cfg.window_height, 760);
        assert!(!cfg.watch_file_changes);
        assert!(cfg.colors.is_none());
    }

    #[test]
    fn test_load_config_missing_vs_explicit_cli_missing() {
        let dir = TestTempDir::new("missing");
        let missing = dir.path.join("not_exists.json");

        let default_res = load_config(Some(&missing));
        assert_eq!(default_res.config, AppConfig::default());
        assert!(default_res.source_path.is_none());
        assert!(default_res.warning.is_none());
        assert!(!default_res.source_is_corrupt);

        let cli_res = load_config_with_source(Some(&missing), true);
        assert_eq!(cli_res.config, AppConfig::default());
        assert!(cli_res.source_path.is_none());
        assert!(cli_res.warning.is_some());
        assert!(!cli_res.source_is_corrupt);

        let none_res = load_config(None);
        assert_eq!(none_res.config, AppConfig::default());
        assert!(none_res.source_path.is_none());
        assert!(none_res.warning.is_none());
        assert!(!none_res.source_is_corrupt);
    }

    #[test]
    fn test_load_config_empty_whitespace_and_bom_only_are_corrupt() {
        let dir = TestTempDir::new("empty_corrupt");

        for (name, content) in [
            ("empty.json", "".as_bytes()),
            ("spaces.json", b"   \r\n\t  "),
            ("bom_only.json", &[0xEF, 0xBB, 0xBF]),
        ] {
            let p = dir.path.join(name);
            fs::write(&p, content).unwrap();
            let res = load_config(Some(&p));
            assert_eq!(res.config, AppConfig::default());
            assert_eq!(res.source_path.as_deref(), Some(p.as_path()));
            assert!(res.warning.is_some());
            assert!(res.source_is_corrupt, "{name} 应标记 source_is_corrupt");
        }
    }

    #[test]
    fn test_load_config_syntax_error_and_non_object_root_are_corrupt() {
        let dir = TestTempDir::new("syntax_corrupt");

        for (name, body) in [
            ("bad_syntax.json", "{ \"theme\": \"dark\", }"),
            ("nan.json", "{ \"fontSize\": NaN }"),
            ("array.json", "[1, 2, 3]"),
            ("number.json", "123"),
            ("string.json", "\"dark\""),
            ("null.json", "null"),
            ("bool.json", "true"),
        ] {
            let p = dir.path.join(name);
            fs::write(&p, body).unwrap();
            let res = load_config(Some(&p));
            assert_eq!(res.config, AppConfig::default());
            assert_eq!(res.source_path.as_deref(), Some(p.as_path()));
            assert!(res.warning.is_some());
            assert!(res.source_is_corrupt, "{name} 应标记 source_is_corrupt");
        }
    }

    #[test]
    fn test_load_config_utf8_bom_and_snake_case_aliases_supported() {
        let dir = TestTempDir::new("bom_snake");
        let p = dir.path.join("config.json");
        let body = "\u{FEFF}{\n\
            \"theme\": \"dark\",\n\
            \"font_family\": \"Noto Sans SC\",\n\
            \"mono_font_family\": \"JetBrains Mono\",\n\
            \"font_size\": 18.5,\n\
            \"line_height\": 1.8,\n\
            \"max_width\": 900,\n\
            \"page_padding\": 48,\n\
            \"editor_font_size\": 16.5,\n\
            \"editor_line_height\": 1.7,\n\
            \"editor_tab_size\": 2,\n\
            \"show_line_numbers\": false,\n\
            \"word_wrap\": false,\n\
            \"remember_window_geometry\": false,\n\
            \"window_width\": 1280,\n\
            \"window_height\": 800,\n\
            \"watch_file_changes\": true,\n\
            \"colors\": {\n\
                \"code_background\": \"#1E1E1E\",\n\
                \"quote_background\": \"#252526\"\n\
            }\n\
        }";
        fs::write(&p, body).unwrap();

        let res = load_config(Some(&p));
        assert!(res.warning.is_none(), "意外警告: {:?}", res.warning);
        assert!(!res.source_is_corrupt);
        assert_eq!(res.config.theme, ThemeMode::Dark);
        assert_eq!(res.config.font_family, "Noto Sans SC");
        assert_eq!(res.config.mono_font_family, "JetBrains Mono");
        assert_eq!(res.config.font_size, 18.5);
        assert_eq!(res.config.line_height, 1.8);
        assert_eq!(res.config.max_width, 900);
        assert_eq!(res.config.page_padding, 48);
        assert_eq!(res.config.editor_font_size, 16.5);
        assert_eq!(res.config.editor_line_height, 1.7);
        assert_eq!(res.config.editor_tab_size, 2);
        assert!(!res.config.show_line_numbers);
        assert!(!res.config.word_wrap);
        assert!(!res.config.remember_window_geometry);
        assert_eq!(res.config.window_width, 1280);
        assert_eq!(res.config.window_height, 800);
        assert!(res.config.watch_file_changes);
        let colors = res.config.colors.expect("应解析 colors");
        assert_eq!(colors.code_background.as_deref(), Some("#1E1E1E"));
        assert_eq!(colors.quote_background.as_deref(), Some("#252526"));
    }

    #[test]
    fn test_load_config_float_boundaries_exact_without_f32_rounding_false_positive() {
        let dir = TestTempDir::new("float_bounds");
        let p = dir.path.join("bounds.json");
        fs::write(
            &p,
            r#"{
                "fontSize": 12.0,
                "lineHeight": 1.2,
                "editorFontSize": 28.0,
                "editorLineHeight": 2.5
            }"#,
        )
        .unwrap();

        let res = load_config(Some(&p));
        assert!(res.warning.is_none(), "合法边界不应报警: {:?}", res.warning);
        assert_eq!(res.config.font_size, 12.0);
        assert_eq!(res.config.line_height, 1.2);
        assert_eq!(res.config.editor_font_size, 28.0);
        assert_eq!(res.config.editor_line_height, 2.5);
    }

    #[test]
    fn test_load_config_theme_case_insensitive_and_invalid_fallback() {
        let dir = TestTempDir::new("theme");
        let p_valid = dir.path.join("valid_theme.json");
        fs::write(&p_valid, r#"{"theme": "  SyStEm  "}"#).unwrap();
        let res_valid = load_config(Some(&p_valid));
        assert_eq!(res_valid.config.theme, ThemeMode::System);
        assert!(res_valid.warning.is_none());

        let p_invalid = dir.path.join("invalid_theme.json");
        fs::write(&p_invalid, r#"{"theme": "sepia", "maxWidth": 820}"#).unwrap();
        let res_invalid = load_config(Some(&p_invalid));
        assert_eq!(res_invalid.config.theme, ThemeMode::Light);
        assert_eq!(res_invalid.config.max_width, 820);
        assert!(res_invalid.warning.is_some());
        assert!(!res_invalid.source_is_corrupt);
    }

    #[test]
    fn test_load_config_numeric_type_mismatch_and_overflow_fallback() {
        let dir = TestTempDir::new("type_mismatch");
        let p = dir.path.join("mismatch.json");
        fs::write(
            &p,
            r#"{
                "theme": "dark",
                "fontSize": "18",
                "lineHeight": 1e40,
                "maxWidth": 760.5,
                "pagePadding": -1,
                "showLineNumbers": 1,
                "windowWidth": 1024
            }"#,
        )
        .unwrap();

        let res = load_config(Some(&p));
        assert!(!res.source_is_corrupt);
        assert!(res.warning.is_some());
        assert_eq!(res.config.theme, ThemeMode::Dark);
        assert_eq!(res.config.window_width, 1024);
        assert_eq!(res.config.font_size, 16.0);
        assert_eq!(res.config.line_height, 1.65);
        assert_eq!(res.config.max_width, 760);
        assert_eq!(res.config.page_padding, 40);
        assert!(res.config.show_line_numbers);
    }

    #[test]
    fn test_load_config_out_of_range_values_fallback_not_clamp() {
        let dir = TestTempDir::new("out_of_range");
        let p = dir.path.join("range.json");
        fs::write(
            &p,
            r#"{
                "fontSize": 11.9,
                "lineHeight": 2.51,
                "maxWidth": 479,
                "pagePadding": 121,
                "editorFontSize": 28.1,
                "editorLineHeight": 1.19,
                "editorTabSize": 9,
                "windowWidth": 399,
                "windowHeight": 2161
            }"#,
        )
        .unwrap();

        let res = load_config(Some(&p));
        let defaults = AppConfig::default();
        assert!(!res.source_is_corrupt);
        assert!(res.warning.is_some());
        assert_eq!(res.config.font_size, defaults.font_size);
        assert_eq!(res.config.line_height, defaults.line_height);
        assert_eq!(res.config.max_width, defaults.max_width);
        assert_eq!(res.config.page_padding, defaults.page_padding);
        assert_eq!(res.config.editor_font_size, defaults.editor_font_size);
        assert_eq!(res.config.editor_line_height, defaults.editor_line_height);
        assert_eq!(res.config.editor_tab_size, defaults.editor_tab_size);
        assert_eq!(res.config.window_width, defaults.window_width);
        assert_eq!(res.config.window_height, defaults.window_height);
    }

    #[test]
    fn test_load_config_string_length_and_whitespace_validation() {
        let dir = TestTempDir::new("string_validation");
        let too_long = "A".repeat(257);
        let p = dir.path.join("strings.json");
        fs::write(
            &p,
            format!(
                r#"{{
                    "fontFamily": "   ",
                    "monoFontFamily": "{too_long}",
                    "maxWidth": 800
                }}"#
            ),
        )
        .unwrap();

        let res = load_config(Some(&p));
        let defaults = AppConfig::default();
        assert_eq!(res.config.font_family, defaults.font_family);
        assert_eq!(res.config.mono_font_family, defaults.mono_font_family);
        assert_eq!(res.config.max_width, 800);
        assert!(res.warning.is_some());

        let p2 = dir.path.join("strings_trim.json");
        fs::write(&p2, r#"{"fontFamily": "  Segoe UI  "}"#).unwrap();
        let res2 = load_config(Some(&p2));
        assert_eq!(res2.config.font_family, "Segoe UI");
        assert!(res2.warning.is_none());
    }

    #[test]
    fn test_load_config_custom_colors_hex_validation_partial_fallback() {
        let dir = TestTempDir::new("colors");
        let p = dir.path.join("colors.json");
        fs::write(
            &p,
            r##"{
                "colors": {
                    "background": "#1a2B3c",
                    "text": "123456",
                    "codeBackground": "#123",
                    "quoteBackground": "#GGGGGG",
                    "link": 999,
                    "border": "#ABCDEF"
                }
            }"##,
        )
        .unwrap();

        let res = load_config(Some(&p));
        assert!(!res.source_is_corrupt);
        assert!(res.warning.is_some());
        let colors = res.config.colors.expect("有效色值应保留");
        assert_eq!(colors.background.as_deref(), Some("#1a2B3c"));
        assert!(colors.text.is_none());
        assert!(colors.code_background.is_none());
        assert!(colors.quote_background.is_none());
        assert!(colors.link.is_none());
        assert_eq!(colors.border.as_deref(), Some("#ABCDEF"));

        // colors 非 Object 时整体回退 None
        let p_non_obj = dir.path.join("colors_arr.json");
        fs::write(&p_non_obj, r##"{"colors": ["#112233"]}"##).unwrap();
        let res2 = load_config(Some(&p_non_obj));
        assert!(res2.config.colors.is_none());
        assert!(res2.warning.is_some());
    }

    #[test]
    fn test_config_watcher_detects_create_modify_and_delete() {
        let dir = TestTempDir::new("watcher");
        let p = dir.path.join("watch.json");

        let mut watcher = ConfigWatcher::new(Some(p.clone()));
        assert_eq!(watcher.path(), Some(p.as_path()));
        assert!(!watcher.poll_changed());
        assert!(watcher.reload_if_changed().is_none());

        // 创建文件
        fs::write(&p, r#"{"theme":"dark"}"#).unwrap();
        let reloaded = watcher.reload_if_changed().expect("应检测到文件创建");
        assert_eq!(reloaded.config.theme, ThemeMode::Dark);
        assert!(!watcher.poll_changed());

        // 修改大小与内容
        fs::write(&p, r#"{"theme":"system","maxWidth":880}"#).unwrap();
        let reloaded2 = watcher.reload_if_changed().expect("应检测到文件修改");
        assert_eq!(reloaded2.config.theme, ThemeMode::System);
        assert_eq!(reloaded2.config.max_width, 880);

        // 删除文件
        fs::remove_file(&p).unwrap();
        let reloaded3 = watcher.reload_if_changed().expect("应检测到文件删除");
        assert_eq!(reloaded3.config, AppConfig::default());

        // force_reload 即使未改变也能返回当前加载结果
        fs::write(&p, r#"{"maxWidth":640}"#).unwrap();
        let forced = watcher.force_reload();
        assert_eq!(forced.config.max_width, 640);
        assert!(!watcher.poll_changed());
    }

    #[test]
    fn test_save_geometry_refuses_corrupt_and_runtime_corrupted_file() {
        let dir = TestTempDir::new("save_corrupt");
        let p = dir.path.join("corrupt.json");
        let bad_bytes = b"{ broken json !!!";
        fs::write(&p, bad_bytes).unwrap();

        let load_bad = load_config(Some(&p));
        assert!(load_bad.source_is_corrupt);
        let err = save_window_geometry_if_valid(&load_bad, &p, 1024, 768)
            .expect_err("损坏配置必须禁止覆写");
        assert!(matches!(err, ConfigSaveError::CorruptConfigProtected));
        assert!(err.source().is_none());
        assert_eq!(fs::read(&p).unwrap(), bad_bytes);

        // 运行期初始合法，但磁盘被改坏后尝试保存
        let valid_p = dir.path.join("runtime_corrupt.json");
        fs::write(&valid_p, r#"{"theme":"dark"}"#).unwrap();
        let load_ok = load_config(Some(&valid_p));
        assert!(!load_ok.source_is_corrupt);

        fs::write(&valid_p, bad_bytes).unwrap();
        let err2 = save_window_geometry_if_valid(&load_ok, &valid_p, 1024, 768)
            .expect_err("运行期被改坏后必须拒绝覆写");
        assert!(matches!(err2, ConfigSaveError::CorruptConfigProtected));
        assert_eq!(fs::read(&valid_p).unwrap(), bad_bytes);
    }

    #[test]
    fn test_save_geometry_skips_when_disabled_or_invalid_dimensions() {
        let dir = TestTempDir::new("save_skip");
        let p = dir.path.join("cfg.json");
        fs::write(&p, r#"{"rememberWindowGeometry": false}"#).unwrap();
        let res = load_config(Some(&p));
        assert!(!save_window_geometry_if_valid(&res, &p, 1024, 768).unwrap());

        let mut enabled = ConfigLoadResult {
            config: AppConfig::default(),
            source_path: Some(p.clone()),
            warning: None,
            source_is_corrupt: false,
        };
        enabled.config.remember_window_geometry = true;
        let err = save_window_geometry_if_valid(&enabled, &p, 0, 0).unwrap_err();
        assert!(matches!(
            err,
            ConfigSaveError::InvalidGeometry { width: 0, height: 0 }
        ));
    }

    #[test]
    fn test_save_geometry_preserves_other_fields_and_unknown_keys() {
        let dir = TestTempDir::new("save_preserve");
        let p = dir.path.join("preserve.json");
        fs::write(
            &p,
            r##"{
                "theme": "dark",
                "window_width": 800,
                "fontSize": 999,
                "customNote": "keep-me",
                "colors": { "background": "#101010" }
            }"##,
        )
        .unwrap();

        let loaded = load_config(Some(&p));
        assert!(!loaded.source_is_corrupt);
        assert!(save_window_geometry_if_valid(&loaded, &p, 1180, 820).unwrap());

        let raw_after = fs::read_to_string(&p).unwrap();
        let val: Value = serde_json::from_str(&raw_after).unwrap();
        let obj = val.as_object().unwrap();
        assert_eq!(obj.get("theme").and_then(Value::as_str), Some("dark"));
        assert_eq!(obj.get("windowWidth").and_then(Value::as_u64), Some(1180));
        assert_eq!(obj.get("windowHeight").and_then(Value::as_u64), Some(820));
        assert!(!obj.contains_key("window_width"));
        assert_eq!(obj.get("fontSize").and_then(Value::as_u64), Some(999));
        assert_eq!(obj.get("customNote").and_then(Value::as_str), Some("keep-me"));
        assert_eq!(
            obj.get("colors")
                .and_then(|c| c.get("background"))
                .and_then(Value::as_str),
            Some("#101010")
        );

        // 目标文件尚不存在时允许新建落盘
        let new_p = dir.path.join("nested").join("new_config.json");
        let missing_res = load_config(Some(&new_p));
        assert!(save_window_geometry_if_valid(&missing_res, &new_p, 1100, 780).unwrap());
        let reloaded = load_config(Some(&new_p));
        assert_eq!(reloaded.config.window_width, 1100);
        assert_eq!(reloaded.config.window_height, 780);
    }

    #[test]
    fn test_save_geometry_readonly_file_fails_safely_without_temp_residue() {
        let dir = TestTempDir::new("save_readonly");
        let p = dir.path.join("readonly.json");
        let original = "{\n  \"theme\": \"dark\",\n  \"windowWidth\": 960\n}\n";
        fs::write(&p, original).unwrap();

        let loaded = load_config(Some(&p));
        let mut perms = fs::metadata(&p).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&p, perms).unwrap();

        let err = save_window_geometry_if_valid(&loaded, &p, 1200, 900)
            .expect_err("只读配置必须返回写入失败");
        assert!(matches!(err, ConfigSaveError::Storage(StorageError::Io(_))));
        assert!(err.source().is_some());
        assert_eq!(fs::read_to_string(&p).unwrap(), original);

        let entries: Vec<PathBuf> = fs::read_dir(&dir.path)
            .unwrap()
            .filter_map(|e| e.ok().map(|item| item.path()))
            .collect();
        assert_eq!(entries, vec![p]);
    }
}
