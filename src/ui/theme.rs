//! 主题调色板、Windows 系统深浅色感知与自定义色值解析模块（归属 Milestone 4）。
//!
//! 视觉配色对标 `mdfiler.com` 的柔和护眼高对比度美学：
//! - 亮色主题：暖白纸感底色 `#FAF8F5` + 深炭黑正文 `#1F2328`（WCAG 对比度 > 14:1）；
//! - 暗色主题：低眩光深灰底色 `#1E1E1E` + 柔白正文 `#E4E4E7`（WCAG 对比度 > 13:1）；
//! - 系统主题：实时读取 `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme`；
//! - 支持 `config.json` 中 `colors` 覆盖，并自动保持未指定衍生色的协调性。

use windows::Win32::Foundation::{COLORREF, WIN32_ERROR};
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, REG_VALUE_TYPE, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
};
use windows::core::PCWSTR;

use crate::core::config::{AppConfig, ThemeMode};
use crate::core::markdown::TokenKind;

/// 24 位 RGB 颜色结构。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl RgbColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// 解析 `#RRGGBB` 6 位十六进制颜色字符串。
    pub fn from_hex(hex: &str) -> Option<Self> {
        let s = hex.trim();
        if s.len() != 7 || !s.starts_with('#') {
            return None;
        }
        let r = u8::from_str_radix(&s[1..3], 16).ok()?;
        let g = u8::from_str_radix(&s[3..5], 16).ok()?;
        let b = u8::from_str_radix(&s[5..7], 16).ok()?;
        Some(Self { r, g, b })
    }

    /// 转换为 Direct2D 浮点色值（支持自定义不透明度 `alpha`）。
    pub fn to_d2d(self, alpha: f32) -> D2D1_COLOR_F {
        D2D1_COLOR_F {
            r: f32::from(self.r) / 255.0,
            g: f32::from(self.g) / 255.0,
            b: f32::from(self.b) / 255.0,
            a: alpha,
        }
    }

    /// 转换为 Win32 GDI / RichEdit 使用的 `COLORREF` (`0x00BBGGRR`)。
    pub const fn to_colorref(self) -> COLORREF {
        COLORREF(
            (self.r as u32) | ((self.g as u32) << 8) | ((self.b as u32) << 16),
        )
    }

    /// 计算 WCAG 2.1 相对亮度（0.0 ~ 1.0）。
    pub fn relative_luminance(self) -> f64 {
        fn channel(v: u8) -> f64 {
            let c = f64::from(v) / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }

    /// 计算两种颜色之间的 WCAG 对比度比值（1.0 ~ 21.0）。
    pub fn contrast_ratio(self, other: Self) -> f64 {
        let l1 = self.relative_luminance();
        let l2 = other.relative_luminance();
        let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// 线性插值混合两种颜色（`t = 0.0` 返回自身，`t = 1.0` 返回 `other`）。
    pub fn blend(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let lerp = |a: u8, b: u8| -> u8 {
            ((f32::from(a) * (1.0 - t)) + (f32::from(b) * t)).round() as u8
        };
        Self {
            r: lerp(self.r, other.r),
            g: lerp(self.g, other.g),
            b: lerp(self.b, other.b),
        }
    }
}

/// 完整阅读器与编辑区主题调色板。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemePalette {
    pub is_dark: bool,
    pub background: RgbColor,
    pub toolbar_bg: RgbColor,
    pub toolbar_border: RgbColor,
    pub text: RgbColor,
    pub heading_text: RgbColor,
    pub muted_text: RgbColor,
    pub code_bg: RgbColor,
    pub code_border: RgbColor,
    pub quote_bg: RgbColor,
    pub quote_bar: RgbColor,
    pub link: RgbColor,
    pub border: RgbColor,
    pub table_header_bg: RgbColor,
    pub table_stripe_bg: RgbColor,
    pub selection_bg: RgbColor,
    pub find_match_bg: RgbColor,
    pub find_active_bg: RgbColor,
    pub error_text: RgbColor,
    pub tok_plain: RgbColor,
    pub tok_keyword: RgbColor,
    pub tok_string: RgbColor,
    pub tok_comment: RgbColor,
    pub tok_number: RgbColor,
    pub tok_type_or_fn: RgbColor,
}

impl ThemePalette {
    /// 默认亮色纸感主题（对标 mdfiler.com 柔和暖白美学）。
    pub const fn light() -> Self {
        Self {
            is_dark: false,
            background: RgbColor::new(0xFA, 0xF8, 0xF5),
            toolbar_bg: RgbColor::new(0xF3, 0xF0, 0xEA),
            toolbar_border: RgbColor::new(0xE2, 0xDD, 0xD5),
            text: RgbColor::new(0x1F, 0x23, 0x28),
            heading_text: RgbColor::new(0x11, 0x18, 0x27),
            muted_text: RgbColor::new(0x4B, 0x55, 0x63),
            code_bg: RgbColor::new(0xF1, 0xEF, 0xEA),
            code_border: RgbColor::new(0xDF, 0xDB, 0xD2),
            quote_bg: RgbColor::new(0xF4, 0xF1, 0xEA),
            quote_bar: RgbColor::new(0xB4, 0xA9, 0x9A),
            link: RgbColor::new(0x09, 0x69, 0xDA),
            border: RgbColor::new(0xD0, 0xD7, 0xDE),
            table_header_bg: RgbColor::new(0xEF, 0xEC, 0xE5),
            table_stripe_bg: RgbColor::new(0xF6, 0xF4, 0xEF),
            selection_bg: RgbColor::new(0x93, 0xC5, 0xFD),
            find_match_bg: RgbColor::new(0xFD, 0xE6, 0x8A),
            find_active_bg: RgbColor::new(0xF5, 0x9E, 0x0B),
            error_text: RgbColor::new(0xB9, 0x1C, 0x1C),
            tok_plain: RgbColor::new(0x24, 0x29, 0x2F),
            tok_keyword: RgbColor::new(0xCF, 0x22, 0x2E),
            tok_string: RgbColor::new(0x0A, 0x30, 0x69),
            tok_comment: RgbColor::new(0x6E, 0x77, 0x81),
            tok_number: RgbColor::new(0x05, 0x50, 0xAE),
            tok_type_or_fn: RgbColor::new(0x82, 0x50, 0xDF),
        }
    }

    /// 默认暗色低眩光护眼主题。
    pub const fn dark() -> Self {
        Self {
            is_dark: true,
            background: RgbColor::new(0x1E, 0x1E, 0x1E),
            toolbar_bg: RgbColor::new(0x25, 0x25, 0x29),
            toolbar_border: RgbColor::new(0x36, 0x36, 0x3D),
            text: RgbColor::new(0xE4, 0xE4, 0xE7),
            heading_text: RgbColor::new(0xF4, 0xF4, 0xF5),
            muted_text: RgbColor::new(0xB8, 0xB8, 0xC2),
            code_bg: RgbColor::new(0x2A, 0x2A, 0x2E),
            code_border: RgbColor::new(0x3F, 0x3F, 0x46),
            quote_bg: RgbColor::new(0x25, 0x25, 0x29),
            quote_bar: RgbColor::new(0x52, 0x52, 0x5B),
            link: RgbColor::new(0x58, 0xA6, 0xFF),
            border: RgbColor::new(0x3F, 0x3F, 0x46),
            table_header_bg: RgbColor::new(0x2A, 0x2A, 0x30),
            table_stripe_bg: RgbColor::new(0x23, 0x23, 0x27),
            selection_bg: RgbColor::new(0x26, 0x4F, 0x78),
            find_match_bg: RgbColor::new(0x71, 0x3F, 0x12),
            find_active_bg: RgbColor::new(0xD9, 0x77, 0x06),
            error_text: RgbColor::new(0xF8, 0x71, 0x71),
            tok_plain: RgbColor::new(0xE4, 0xE4, 0xE7),
            tok_keyword: RgbColor::new(0xFF, 0x7B, 0x72),
            tok_string: RgbColor::new(0xA5, 0xD6, 0xFF),
            tok_comment: RgbColor::new(0x8B, 0x94, 0x9E),
            tok_number: RgbColor::new(0x79, 0xC0, 0xFF),
            tok_type_or_fn: RgbColor::new(0xD2, 0xA8, 0xFF),
        }
    }

    /// 按 `AppConfig` 解析基础主题并叠加自定义 `colors` 覆盖。
    pub fn from_config(cfg: &AppConfig) -> Self {
        let use_dark = match cfg.theme {
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
            ThemeMode::System => detect_system_is_dark(),
        };
        let mut p = if use_dark { Self::dark() } else { Self::light() };

        if let Some(custom) = &cfg.colors {
            if let Some(c) = custom.background.as_deref().and_then(RgbColor::from_hex) {
                p.background = c;
                let bg_dark = c.relative_luminance() < 0.25;
                p.is_dark = bg_dark;
                p.toolbar_bg = if bg_dark {
                    c.blend(RgbColor::new(0xFF, 0xFF, 0xFF), 0.05)
                } else {
                    c.blend(RgbColor::new(0x00, 0x00, 0x00), 0.04)
                };
                p.table_stripe_bg = if bg_dark {
                    c.blend(RgbColor::new(0xFF, 0xFF, 0xFF), 0.03)
                } else {
                    c.blend(RgbColor::new(0x00, 0x00, 0x00), 0.025)
                };
            }
            if let Some(c) = custom.text.as_deref().and_then(RgbColor::from_hex) {
                p.text = c;
                p.heading_text = c;
                p.tok_plain = c;
            }
            if let Some(c) = custom.code_background.as_deref().and_then(RgbColor::from_hex) {
                p.code_bg = c;
                p.table_header_bg = c;
            }
            if let Some(c) = custom.quote_background.as_deref().and_then(RgbColor::from_hex) {
                p.quote_bg = c;
            }
            if let Some(c) = custom.link.as_deref().and_then(RgbColor::from_hex) {
                p.link = c;
            }
            if let Some(c) = custom.border.as_deref().and_then(RgbColor::from_hex) {
                p.border = c;
                p.code_border = c;
                p.toolbar_border = c;
            }
        }
        p
    }

    /// 获取代码词法高亮 Token 对应前景色。
    pub const fn token_color(&self, token: TokenKind) -> RgbColor {
        match token {
            TokenKind::Plain => self.tok_plain,
            TokenKind::Keyword => self.tok_keyword,
            TokenKind::String => self.tok_string,
            TokenKind::Comment => self.tok_comment,
            TokenKind::Number => self.tok_number,
            TokenKind::TypeOrFn => self.tok_type_or_fn,
        }
    }
}

/// 读取 Windows 注册表 `AppsUseLightTheme` 判断系统当前是否处于深色应用模式。
pub fn detect_system_is_dark() -> bool {
    let subkey: Vec<u16> = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let val_name: Vec<u16> = "AppsUseLightTheme"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let mut hkey = HKEY::default();
    let open = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            KEY_READ,
            &mut hkey,
        )
    };
    if open != WIN32_ERROR(0) {
        return false;
    }

    let mut kind = REG_VALUE_TYPE::default();
    let mut data: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegQueryValueExW(
            hkey,
            PCWSTR(val_name.as_ptr()),
            None,
            Some(&mut kind),
            Some((&mut data as *mut u32).cast::<u8>()),
            Some(&mut size),
        )
    };
    let _ = unsafe { RegCloseKey(hkey) };
    if status == WIN32_ERROR(0) {
        data == 0
    } else {
        false
    }
}

/// 从逗号分隔字体回退链中提取第一个具体的 DirectWrite 字体族名（跳过 CSS 泛族名）。
pub fn primary_font_family<'a>(chain: &'a str, fallback: &'a str) -> &'a str {
    for raw in chain.split(',') {
        let name = raw.trim().trim_matches('"').trim_matches('\'');
        if name.is_empty() {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "sans-serif" | "serif" | "monospace" | "system-ui" | "ui-monospace"
        ) {
            continue;
        }
        return name;
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::{RgbColor, ThemePalette, primary_font_family};
    use crate::core::config::{AppConfig, CustomColors, ThemeMode};

    #[test]
    fn test_default_light_and_dark_meet_wcag_aaa() {
        let light = ThemePalette::light();
        let dark = ThemePalette::dark();
        assert!(light.text.contrast_ratio(light.background) >= 7.0);
        assert!(dark.text.contrast_ratio(dark.background) >= 7.0);
        assert!(light.heading_text.contrast_ratio(light.background) >= 7.0);
        assert!(dark.heading_text.contrast_ratio(dark.background) >= 7.0);
    }

    #[test]
    fn test_custom_colors_override_selectively() {
        let mut cfg = AppConfig::default();
        cfg.theme = ThemeMode::Light;
        cfg.colors = Some(CustomColors {
            background: Some("#101418".to_string()),
            text: Some("#F0F4F8".to_string()),
            code_background: None,
            quote_background: None,
            link: Some("#38BDF8".to_string()),
            border: None,
        });
        let pal = ThemePalette::from_config(&cfg);
        assert_eq!(pal.background, RgbColor::new(0x10, 0x14, 0x18));
        assert_eq!(pal.text, RgbColor::new(0xF0, 0xF4, 0xF8));
        assert_eq!(pal.link, RgbColor::new(0x38, 0xBD, 0xF8));
        assert!(pal.is_dark);
    }

    #[test]
    fn test_primary_font_family_extraction() {
        assert_eq!(
            primary_font_family("Microsoft YaHei UI, Segoe UI, sans-serif", "Segoe UI"),
            "Microsoft YaHei UI"
        );
        assert_eq!(
            primary_font_family("sans-serif, monospace", "Segoe UI"),
            "Segoe UI"
        );
        assert_eq!(
            primary_font_family("\"Cascadia Code\", Consolas, monospace", "Consolas"),
            "Cascadia Code"
        );
    }
}
