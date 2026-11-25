//! 基于系统原生 `Msftedit.dll`（`RICHEDIT50W`）的轻量纯文本修改模式编辑区（归属 Milestone 5）。
//!
//! 特点与契约：
//! - 零第三方编辑器控件开销，原生支持中文 TSF 输入法（IME）、多级撤销/重做（`Ctrl+Z`/`Ctrl+Y`）
//!   与 UIA `TextPattern`/`ValuePattern`；
//! - 强制 `TM_PLAINTEXT` 纯文本模式，粘贴外部富文本时自动去格式；
//! - 借助 `EM_SETRECT` 居中对齐版心宽度，配合 `EM_SETBKGNDCOLOR` 与 `CHARFORMAT2W` 完整同步
//!   亮/暗/自定义主题色与等宽字体，实现阅读态 ↔ 修改态的平滑视觉连续性；
//! - 突破默认文本长度限制（`EM_EXLIMITTEXT`），流畅编辑 200KB+ 长文档。

use std::ffi::c_void;
use std::sync::OnceLock;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, POINTL, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::LoadLibraryW;
use windows::Win32::UI::Controls::{
    EM_CHARFROMPOS, EM_EMPTYUNDOBUFFER, EM_GETFIRSTVISIBLELINE, EM_LINESCROLL, EM_REPLACESEL,
    EM_SETMODIFY, EM_SETRECT, EM_SETSEL,
};
use windows::Win32::UI::Controls::RichEdit::{
    CFM_COLOR, CFM_FACE, CFM_SIZE, CHARFORMAT2W, EM_EXLIMITTEXT, EM_EXLINEFROMCHAR, EM_GETTEXTEX,
    EM_SETBKGNDCOLOR, EM_SETCHARFORMAT, EM_SETEVENTMASK, EM_SETPARAFORMAT, EM_SETTARGETDEVICE,
    EM_SETTEXTMODE, ENM_CHANGE, GETTEXTEX, GT_DEFAULT, PARAFORMAT2, PFM_LINESPACING, PFM_TABSTOPS,
    SCF_ALL, SCF_DEFAULT, TM_MULTICODEPAGE, TM_MULTILEVELUNDO, TM_PLAINTEXT,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, ES_AUTOVSCROLL, ES_MULTILINE, ES_NOHIDESEL, ES_WANTRETURN,
    GetWindowTextLengthW, GetWindowTextW, HMENU, MoveWindow, SW_HIDE, SW_SHOW, SendMessageW,
    SetWindowTextW, ShowWindow, WS_CHILD, WS_VSCROLL,
};
use windows::core::{PCWSTR, w};

use crate::core::config::AppConfig;
use crate::ui::IDC_EDITOR;
use crate::ui::theme::{ThemePalette, primary_font_family};

static MSFTEDIT_LOADED: OnceLock<bool> = OnceLock::new();

fn ensure_msftedit_loaded() {
    MSFTEDIT_LOADED.get_or_init(|| unsafe { LoadLibraryW(w!("Msftedit.dll")).is_ok() });
}

/// 纯文本编辑视图封装。
pub struct EditorView {
    hwnd: HWND,
    config: AppConfig,
    palette: ThemePalette,
    last_width_px: i32,
    last_height_px: i32,
}

impl EditorView {
    /// 创建 `RICHEDIT50W` 编辑子控件（初始默认隐藏，仅进入修改模式时显示）。
    pub fn create(
        parent: HWND,
        instance: HINSTANCE,
        config: &AppConfig,
        palette: ThemePalette,
    ) -> windows::core::Result<Self> {
        ensure_msftedit_loaded();

        let style = WS_CHILD
            | WS_VSCROLL
            | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(
                (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | ES_NOHIDESEL) as u32,
            );

        let hwnd = unsafe {
            CreateWindowExW(
                Default::default(),
                w!("RICHEDIT50W"),
                w!(""),
                style,
                0,
                0,
                800,
                600,
                Some(parent),
                Some(HMENU(IDC_EDITOR as usize as *mut c_void)),
                Some(instance),
                None,
            )?
        };

        unsafe {
            // 突破 32KB/64KB 默认长度上限，允许编辑多 MB Markdown 文档
            SendMessageW(hwnd, EM_EXLIMITTEXT, Some(WPARAM(0)), Some(LPARAM(0x0100_0000)));
            // 启用纯文本 + 多级撤销模式
            let mode = (TM_PLAINTEXT.0 | TM_MULTILEVELUNDO.0 | TM_MULTICODEPAGE.0) as usize;
            SendMessageW(hwnd, EM_SETTEXTMODE, Some(WPARAM(mode)), Some(LPARAM(0)));
            // 订阅 EN_CHANGE 用于实时追踪脏标记 `*`
            SendMessageW(
                hwnd,
                EM_SETEVENTMASK,
                Some(WPARAM(0)),
                Some(LPARAM(ENM_CHANGE as isize)),
            );
        }

        let mut view = Self {
            hwnd,
            config: config.clone(),
            palette,
            last_width_px: 800,
            last_height_px: 600,
        };
        view.apply_config(config, palette);
        Ok(view)
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// 切换可见性（阅读态隐藏、修改态显示并自动聚焦）。
    pub fn set_visible(&self, visible: bool) {
        unsafe {
            let _ = ShowWindow(self.hwnd, if visible { SW_SHOW } else { SW_HIDE });
            if visible {
                let _ = SetFocus(Some(self.hwnd));
            }
        }
    }

    /// 调整编辑区位置尺寸并同步居中留白矩形（`EM_SETRECT`）。
    pub fn set_bounds(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.last_width_px = w.max(1);
        self.last_height_px = h.max(1);
        unsafe {
            let _ = MoveWindow(self.hwnd, x, y, self.last_width_px, self.last_height_px, true);
        }
        self.update_centered_rect();
    }

    /// 同步配置与主题色（背景、文字、等宽字体、字号、行高、Tab 宽度、自动折行与居中留白）。
    pub fn apply_config(&mut self, config: &AppConfig, palette: ThemePalette) {
        self.config = config.clone();
        self.palette = palette;

        unsafe {
            // 1. 背景色
            SendMessageW(
                self.hwnd,
                EM_SETBKGNDCOLOR,
                Some(WPARAM(0)),
                Some(LPARAM(palette.background.to_colorref().0 as isize)),
            );

            // 2. 自动折行（0 表示折行到窗口宽度，1 表示不自动折行）
            let wrap_flag = if config.word_wrap { 0isize } else { 1isize };
            SendMessageW(
                self.hwnd,
                EM_SETTARGETDEVICE,
                Some(WPARAM(0)),
                Some(LPARAM(wrap_flag)),
            );
        }

        self.apply_text_formatting();
        self.update_centered_rect();
    }

    /// 对当前全部文本与默认输入样式应用清晰和谐的字体、颜色、行高与制表位。
    ///
    /// 若配置为纯西文等宽字体（如 Cascadia Code / Consolas），在 RichEdit 中文场景下系统会强制
    /// 回退到极细衬线宋体（SimSun）造成中英割裂；此时优先与阅读区保持一致使用 `Microsoft YaHei UI`
    /// 确保中英文字重高度协调统一；若用户自定义了自带中文字形的等宽字体（如 Sarasa / YaHei Mono）
    /// 则尊重原字体配置。
    fn apply_text_formatting(&self) {
        let mut cf = CHARFORMAT2W::default();
        cf.Base.cbSize = std::mem::size_of::<CHARFORMAT2W>() as u32;
        cf.Base.dwMask = CFM_FACE | CFM_SIZE | CFM_COLOR;
        cf.Base.dwEffects = Default::default(); // 清除 CFE_AUTOCOLOR，确保暗色主题浅字生效
        cf.Base.yHeight = (self.config.editor_font_size * 15.0).round() as i32;
        cf.Base.crTextColor = self.palette.text.to_colorref();

        let mono = primary_font_family(&self.config.mono_font_family, "Microsoft YaHei UI");
        let chosen_face = if matches!(
            mono.to_ascii_lowercase().as_str(),
            "cascadia code" | "cascadia mono" | "consolas" | "courier new" | "fira code" | "jetbrains mono"
        ) {
            primary_font_family(&self.config.font_family, "Microsoft YaHei UI")
        } else {
            mono
        };
        let wide: Vec<u16> = chosen_face.encode_utf16().take(31).collect();
        for (dst, &src) in cf.Base.szFaceName.iter_mut().zip(wide.iter()) {
            *dst = src;
        }

        unsafe {
            SendMessageW(
                self.hwnd,
                EM_SETCHARFORMAT,
                Some(WPARAM(SCF_DEFAULT as usize)),
                Some(LPARAM((&cf as *const CHARFORMAT2W) as isize)),
            );
            SendMessageW(
                self.hwnd,
                EM_SETCHARFORMAT,
                Some(WPARAM(SCF_ALL as usize)),
                Some(LPARAM((&cf as *const CHARFORMAT2W) as isize)),
            );
        }

        // 段落行高与制表位（规则 5：dyLineSpacing / 20 为行高倍数）
        let mut pf = PARAFORMAT2::default();
        pf.Base.cbSize = std::mem::size_of::<PARAFORMAT2>() as u32;
        pf.Base.dwMask = PFM_LINESPACING | PFM_TABSTOPS;
        pf.bLineSpacingRule = 5;
        pf.dyLineSpacing = (self.config.editor_line_height * 20.0).round() as i32;

        // 每个制表位宽 = tab_size 个半角字符宽度（约 0.55 * 字号 twips）
        let tab_twips =
            (f32::from(self.config.editor_tab_size) * self.config.editor_font_size * 9.0).round()
                as u32;
        let tab_count = pf.Base.rgxTabs.len().min(32);
        pf.Base.cTabCount = tab_count as i16;
        for i in 0..tab_count {
            pf.Base.rgxTabs[i] = tab_twips * (i as u32 + 1);
        }

        unsafe {
            SendMessageW(
                self.hwnd,
                EM_SETPARAFORMAT,
                Some(WPARAM(0)),
                Some(LPARAM((&pf as *const PARAFORMAT2) as isize)),
            );
        }
    }

    /// 同步计算与阅读模式版心对称的居中 `EM_SETRECT` 内边距。
    fn update_centered_rect(&self) {
        let dpi = unsafe { GetDpiForWindow(self.hwnd) };
        let scale = if dpi > 0 { dpi as f32 / 96.0 } else { 1.0 };
        let w_dip = self.last_width_px as f32 / scale;
        let h_dip = self.last_height_px as f32 / scale;

        let pad = self.config.page_padding as f32;
        let max_w = self.config.max_width as f32;
        let avail = (w_dip - pad * 2.0).max(200.0);
        let content_w = avail.min(max_w).min((w_dip - 24.0).max(160.0));
        let left_dip = ((w_dip - content_w) * 0.5).max(12.0);
        let top_dip = (pad * 0.6).clamp(14.0, 40.0);

        let rc = RECT {
            left: (left_dip * scale).round() as i32,
            top: (top_dip * scale).round() as i32,
            right: ((left_dip + content_w) * scale)
                .round()
                .min((self.last_width_px - 18).max(120) as f32) as i32,
            bottom: ((h_dip - 12.0).max(40.0) * scale).round() as i32,
        };

        unsafe {
            SendMessageW(
                self.hwnd,
                EM_SETRECT,
                Some(WPARAM(0)),
                Some(LPARAM((&rc as *const RECT) as isize)),
            );
        }
    }

    /// 填充文档全文到编辑区并重置撤销栈与修改标志（须由调用方配合 `suppress_change_event` 守卫）。
    pub fn load_text(&self, text_lf: &str) {
        let crlf = text_lf.replace('\n', "\r\n");
        let wide: Vec<u16> = crlf.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let _ = SetWindowTextW(self.hwnd, PCWSTR(wide.as_ptr()));
            SendMessageW(self.hwnd, EM_SETMODIFY, Some(WPARAM(0)), Some(LPARAM(0)));
            SendMessageW(self.hwnd, EM_EMPTYUNDOBUFFER, Some(WPARAM(0)), Some(LPARAM(0)));
        }
        self.apply_text_formatting();
        self.update_centered_rect();
    }

    /// 提取编辑区完整纯文本并统一规范化为内部 `\n` 换行符（单次扫描归一化，避免双倍大字符串拷贝）。
    pub fn get_text_lf(&self) -> String {
        let len = unsafe { GetWindowTextLengthW(self.hwnd) };
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 2];
        let copied = unsafe { GetWindowTextW(self.hwnd, &mut buf) };
        let raw = String::from_utf16_lossy(&buf[..copied.max(0) as usize]);
        if !raw.contains('\r') {
            return raw;
        }
        let mut out = String::with_capacity(raw.len());
        let mut chars = raw.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\r' {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            } else {
                out.push(ch);
            }
        }
        out
    }

    /// 读取 RichEdit 内部原始 UTF-16 缓冲区（`GT_DEFAULT`，段落分隔为单个 `\r`，与字符偏移严格对齐）。
    fn get_internal_utf16(&self) -> Vec<u16> {
        let approx_len = unsafe { GetWindowTextLengthW(self.hwnd) }.max(0) as usize;
        if approx_len == 0 {
            return Vec::new();
        }
        let cap_u16 = approx_len + 16;
        let mut buf = vec![0u16; cap_u16];
        let gt = GETTEXTEX {
            cb: (cap_u16 * std::mem::size_of::<u16>()) as u32,
            flags: GT_DEFAULT,
            codepage: 1200, // UTF-16LE
            lpDefaultChar: windows::core::PCSTR::null(),
            lpUsedDefChar: std::ptr::null_mut(),
        };
        let written = unsafe {
            SendMessageW(
                self.hwnd,
                EM_GETTEXTEX,
                Some(WPARAM((&gt as *const GETTEXTEX) as usize)),
                Some(LPARAM(buf.as_mut_ptr() as isize)),
            )
            .0
        };
        let valid_len = (written.max(0) as usize).min(buf.len());
        buf.truncate(valid_len);
        buf
    }

    /// 返回当前编辑区视口首行对应的 0-based 源码逻辑行号（自动折行下依然精确对齐源码 `\n` 行号）。
    pub fn first_visible_source_line(&self) -> usize {
        let mut rc = RECT::default();
        unsafe {
            SendMessageW(
                self.hwnd,
                windows::Win32::UI::Controls::EM_GETRECT,
                Some(WPARAM(0)),
                Some(LPARAM((&mut rc as *mut RECT) as isize)),
            );
        }
        let pt = POINTL {
            x: rc.left + 2,
            y: rc.top + 4,
        };
        let char_idx = unsafe {
            SendMessageW(
                self.hwnd,
                EM_CHARFROMPOS,
                Some(WPARAM(0)),
                Some(LPARAM((&pt as *const POINTL) as isize)),
            )
            .0
        };
        if char_idx <= 0 {
            return 0;
        }
        let raw_u16 = self.get_internal_utf16();
        let end = (char_idx as usize).min(raw_u16.len());
        let mut line = 0usize;
        let mut i = 0usize;
        while i < end {
            if raw_u16[i] == b'\r' as u16 {
                line += 1;
                if i + 1 < end && raw_u16[i + 1] == b'\n' as u16 {
                    i += 1;
                }
            } else if raw_u16[i] == b'\n' as u16 {
                line += 1;
            }
            i += 1;
        }
        line
    }

    /// 将编辑区滚动到指定的 0-based 源码逻辑行号，并将光标置于该行行首（兼容自动折行）。
    pub fn scroll_to_source_line(&self, target_line: usize) {
        let raw_u16 = self.get_internal_utf16();
        let mut char_pos = 0usize;
        if target_line > 0 && !raw_u16.is_empty() {
            let mut cur_line = 0usize;
            let mut i = 0usize;
            while i < raw_u16.len() {
                if raw_u16[i] == b'\r' as u16 {
                    cur_line += 1;
                    if i + 1 < raw_u16.len() && raw_u16[i + 1] == b'\n' as u16 {
                        i += 1;
                    }
                    if cur_line == target_line {
                        char_pos = i + 1;
                        break;
                    }
                } else if raw_u16[i] == b'\n' as u16 {
                    cur_line += 1;
                    if cur_line == target_line {
                        char_pos = i + 1;
                        break;
                    }
                }
                i += 1;
            }
            if cur_line < target_line {
                char_pos = raw_u16.len();
            }
        }

        unsafe {
            SendMessageW(
                self.hwnd,
                EM_SETSEL,
                Some(WPARAM(char_pos)),
                Some(LPARAM(char_pos as isize)),
            );
            let target_display_line = SendMessageW(
                self.hwnd,
                EM_EXLINEFROMCHAR,
                Some(WPARAM(0)),
                Some(LPARAM(char_pos as isize)),
            )
            .0;
            let first_visible =
                SendMessageW(self.hwnd, EM_GETFIRSTVISIBLELINE, Some(WPARAM(0)), Some(LPARAM(0))).0;
            let delta = target_display_line - first_visible;
            if delta != 0 {
                SendMessageW(
                    self.hwnd,
                    EM_LINESCROLL,
                    Some(WPARAM(0)),
                    Some(LPARAM(delta)),
                );
            }
        }
    }

    /// 插入与 `editorTabSize` 对应的空格缩进（保留撤销栈，支持 `Ctrl+Z` 撤销）。
    pub fn insert_tab_spaces(&self) {
        let spaces = " ".repeat(usize::from(self.config.editor_tab_size.clamp(2, 8)));
        let wide: Vec<u16> = spaces.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            SendMessageW(
                self.hwnd,
                EM_REPLACESEL,
                Some(WPARAM(1)), // fCanUndo = TRUE
                Some(LPARAM(wide.as_ptr() as isize)),
            );
        }
    }
}
