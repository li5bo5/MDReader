//! 左侧极简目录栏组件：顶部双 Tab 切换（「大纲」目录 + 7 天「历史」文件），双缓冲零闪烁自绘。

use std::ffi::c_void;
use std::path::Path;
use std::sync::OnceLock;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CLEARTYPE_QUALITY, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW,
    CreatePen, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER, DT_END_ELLIPSIS,
    DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, EndPaint, FF_DONTCARE,
    FillRect, GetStockObject, HDC, HFONT, HGDIOBJ, InvalidateRect, NULL_PEN, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, PS_SOLID, RoundRect, SRCCOPY, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, GWLP_USERDATA,
    GetClientRect, GetParent, GetWindowLongPtrW, HMENU, IDC_ARROW, IDC_HAND, LoadCursorW,
    MoveWindow, PostMessageW, RegisterClassW, SW_HIDE, SW_SHOW, SetCursor, SetWindowLongPtrW,
    ShowWindow, WM_CREATE, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_NCDESTROY, WM_PAINT, WM_PRINTCLIENT, WM_SETCURSOR, WM_SIZE, WNDCLASSW, WS_CHILD,
    WS_CLIPSIBLINGS, WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

use crate::core::history::{HistoryEntry, current_epoch_secs};
use crate::core::markdown::OutlineItem;
use crate::ui::theme::{RgbColor, ThemePalette};
use crate::ui::{IDC_SIDEBAR, WM_APP_SIDEBAR_HISTORY_OPEN, WM_APP_SIDEBAR_OUTLINE_JUMP};

/// 侧边栏窗口类名。
const SIDEBAR_CLASS_NAME: PCWSTR = w!("MDReaderSidebar");

/// 侧边栏默认逻辑宽度（DIP）。
pub const SIDEBAR_WIDTH_DIP: i32 = 236;

/// 顶部双 Tab 栏高度（DIP）。
const TAB_BAR_HEIGHT_DIP: i32 = 36;

/// 大纲单行高度（DIP）。
const OUTLINE_ROW_HEIGHT_DIP: i32 = 30;

/// 历史单行高度（DIP）。
const HISTORY_ROW_HEIGHT_DIP: i32 = 46;

/// 侧边栏激活标签页。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarTab {
    /// 大纲目录（默认选中）。
    #[default]
    Outline,
    /// 最近 7 天文件历史。
    History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HoverTarget {
    None,
    Tab(SidebarTab),
    Row(usize),
}

struct SidebarState {
    hwnd: HWND,
    palette: ThemePalette,
    tab: SidebarTab,
    outline: Vec<OutlineItem>,
    active_block_index: Option<usize>,
    history: Vec<HistoryEntry>,
    current_file_path: Option<String>,

    scroll_y_px: i32,
    hover: HoverTarget,
    tracking_leave: bool,

    font_normal: HFONT,
    font_bold: HFONT,
    font_small: HFONT,
    cached_dpi: u32,
}

impl Drop for SidebarState {
    fn drop(&mut self) {
        self.free_fonts();
    }
}

/// 左侧目录栏封装句柄。
pub struct SidebarView {
    hwnd: HWND,
    state_ptr: *mut SidebarState,
    visible: bool,
}

static SIDEBAR_CLASS_REGISTERED: OnceLock<bool> = OnceLock::new();

fn register_sidebar_class(instance: HINSTANCE) {
    SIDEBAR_CLASS_REGISTERED.get_or_init(|| unsafe {
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(sidebar_wnd_proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: SIDEBAR_CLASS_NAME,
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);
        true
    });
}

impl SidebarView {
    /// 创建左侧目录栏子窗口（默认展开并选中「大纲」Tab）。
    pub fn create(
        parent: HWND,
        instance: HINSTANCE,
        palette: ThemePalette,
    ) -> windows::core::Result<Self> {
        register_sidebar_class(instance);

        let state = Box::new(SidebarState {
            hwnd: HWND::default(),
            palette,
            tab: SidebarTab::Outline,
            outline: Vec::new(),
            active_block_index: None,
            history: Vec::new(),
            current_file_path: None,
            scroll_y_px: 0,
            hover: HoverTarget::None,
            tracking_leave: false,
            font_normal: HFONT::default(),
            font_bold: HFONT::default(),
            font_small: HFONT::default(),
            cached_dpi: 0,
        });
        let state_ptr = Box::into_raw(state);

        let hwnd = unsafe {
            CreateWindowExW(
                Default::default(),
                SIDEBAR_CLASS_NAME,
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
                0,
                0,
                236,
                600,
                Some(parent),
                Some(HMENU(IDC_SIDEBAR as usize as *mut c_void)),
                Some(instance),
                Some(state_ptr.cast::<c_void>()),
            )?
        };

        unsafe {
            (*state_ptr).hwnd = hwnd;
            (*state_ptr).ensure_fonts();
        }

        Ok(Self {
            hwnd,
            state_ptr,
            visible: true,
        })
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        unsafe {
            let _ = ShowWindow(self.hwnd, if visible { SW_SHOW } else { SW_HIDE });
        }
    }

    pub fn toggle_visible(&mut self) -> bool {
        self.set_visible(!self.visible);
        self.visible
    }

    pub fn set_bounds(&self, x: i32, y: i32, w: i32, h: i32) {
        unsafe {
            let _ = MoveWindow(self.hwnd, x, y, w.max(1), h.max(1), true);
        }
    }

    fn state_mut(&mut self) -> &mut SidebarState {
        unsafe { &mut *self.state_ptr }
    }

    pub fn apply_palette(&mut self, palette: ThemePalette) {
        let s = self.state_mut();
        s.palette = palette;
        s.invalidate();
    }

    pub fn set_outline(&mut self, outline: Vec<OutlineItem>, active_block_index: Option<usize>) {
        let s = self.state_mut();
        s.outline = outline;
        s.active_block_index = active_block_index;
        if s.tab == SidebarTab::Outline {
            s.clamp_scroll();
        }
        s.invalidate();
    }

    pub fn set_active_heading(&mut self, active_block_index: Option<usize>) {
        let s = self.state_mut();
        if s.active_block_index != active_block_index {
            s.active_block_index = active_block_index;
            if s.tab == SidebarTab::Outline {
                s.invalidate();
            }
        }
    }

    pub fn set_history(&mut self, entries: Vec<HistoryEntry>, current_path: Option<&Path>) {
        let s = self.state_mut();
        s.history = entries;
        s.current_file_path = current_path.map(|p| {
            let raw = p.to_string_lossy();
            raw.strip_prefix(r"\\?\")
                .map(str::to_string)
                .unwrap_or_else(|| raw.into_owned())
        });
        if s.tab == SidebarTab::History {
            s.clamp_scroll();
        }
        s.invalidate();
    }
}

impl SidebarState {
    fn dpi(&self) -> u32 {
        let d = unsafe { GetDpiForWindow(self.hwnd) };
        if d > 0 { d } else { 96 }
    }

    fn scale(&self, dip: i32) -> i32 {
        ((dip as f32) * (self.dpi() as f32 / 96.0)).round() as i32
    }

    fn free_fonts(&mut self) {
        unsafe {
            for f in [&mut self.font_normal, &mut self.font_bold, &mut self.font_small] {
                if !f.is_invalid() && !f.0.is_null() {
                    let _ = DeleteObject(HGDIOBJ(f.0));
                    *f = HFONT::default();
                }
            }
        }
    }

    fn ensure_fonts(&mut self) {
        let dpi = self.dpi();
        if self.cached_dpi == dpi && !self.font_normal.is_invalid() && !self.font_normal.0.is_null() {
            return;
        }
        self.free_fonts();
        self.cached_dpi = dpi;
        let s = dpi as f32 / 96.0;
        let face = w!("Microsoft YaHei UI");
        unsafe {
            self.font_normal = CreateFontW(
                -(13.0 * s).round() as i32,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                windows::Win32::Graphics::Gdi::CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                face,
            );
            self.font_bold = CreateFontW(
                -(13.0 * s).round() as i32,
                0,
                0,
                0,
                600,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                windows::Win32::Graphics::Gdi::CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                face,
            );
            self.font_small = CreateFontW(
                -(11.0 * s).round() as i32,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                windows::Win32::Graphics::Gdi::CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                face,
            );
        }
    }

    fn client_size(&self) -> (i32, i32) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        ((rc.right - rc.left).max(1), (rc.bottom - rc.top).max(1))
    }

    fn row_height_px(&self) -> i32 {
        match self.tab {
            SidebarTab::Outline => self.scale(OUTLINE_ROW_HEIGHT_DIP),
            SidebarTab::History => self.scale(HISTORY_ROW_HEIGHT_DIP),
        }
    }

    fn item_count(&self) -> usize {
        match self.tab {
            SidebarTab::Outline => self.outline.len(),
            SidebarTab::History => self.history.len(),
        }
    }

    fn content_height_px(&self) -> i32 {
        (self.item_count() as i32) * self.row_height_px() + self.scale(12)
    }

    fn clamp_scroll(&mut self) {
        let (_, ch) = self.client_size();
        let tab_h = self.scale(TAB_BAR_HEIGHT_DIP);
        let list_h = (ch - tab_h).max(1);
        let max_scroll = (self.content_height_px() - list_h).max(0);
        self.scroll_y_px = self.scroll_y_px.clamp(0, max_scroll);
    }

    fn hit_test(&self, x: i32, y: i32) -> HoverTarget {
        let (cw, ch) = self.client_size();
        if x < 0 || y < 0 || x >= cw || y >= ch {
            return HoverTarget::None;
        }
        let tab_h = self.scale(TAB_BAR_HEIGHT_DIP);
        if y < tab_h {
            if x < cw / 2 {
                return HoverTarget::Tab(SidebarTab::Outline);
            } else {
                return HoverTarget::Tab(SidebarTab::History);
            }
        }
        let count = self.item_count();
        if count == 0 {
            return HoverTarget::None;
        }
        let rel_y = y - tab_h - self.scale(6) + self.scroll_y_px;
        if rel_y < 0 {
            return HoverTarget::None;
        }
        let idx = (rel_y / self.row_height_px().max(1)) as usize;
        if idx < count {
            HoverTarget::Row(idx)
        } else {
            HoverTarget::None
        }
    }

    fn invalidate(&self) {
        if !self.hwnd.is_invalid() && !self.hwnd.0.is_null() {
            let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
        }
    }

    fn sidebar_bg(&self) -> RgbColor {
        if self.palette.is_dark {
            self.palette.toolbar_bg
        } else {
            self.palette.background.blend(self.palette.toolbar_bg, 0.55)
        }
    }

    fn paint_to_dc(&mut self, target_dc: HDC) {
        self.ensure_fonts();
        self.clamp_scroll();
        let (cw, ch) = self.client_size();

        unsafe {
            let mem_dc = CreateCompatibleDC(Some(target_dc));
            if mem_dc.is_invalid() || mem_dc.0.is_null() {
                return;
            }
            let bmp = CreateCompatibleBitmap(target_dc, cw, ch);
            if bmp.is_invalid() || bmp.0.is_null() {
                let _ = DeleteDC(mem_dc);
                return;
            }
            let old_bmp = SelectObject(mem_dc, HGDIOBJ(bmp.0));
            let _ = SetBkMode(mem_dc, TRANSPARENT);

            // 1. 整体背景与右侧 1px 细边框
            let bg_color = self.sidebar_bg();
            let bg_brush = CreateSolidBrush(bg_color.to_colorref());
            let full_rc = RECT {
                left: 0,
                top: 0,
                right: cw,
                bottom: ch,
            };
            FillRect(mem_dc, &full_rc, bg_brush);
            let _ = DeleteObject(HGDIOBJ(bg_brush.0));

            let border_brush = CreateSolidBrush(self.palette.toolbar_border.to_colorref());
            let right_line = RECT {
                left: cw - 1,
                top: 0,
                right: cw,
                bottom: ch,
            };
            FillRect(mem_dc, &right_line, border_brush);

            // 2. 顶部双 Tab 切换栏
            let tab_h = self.scale(TAB_BAR_HEIGHT_DIP);
            let half_w = (cw - 1) / 2;
            let tab_sep = RECT {
                left: 0,
                top: tab_h - 1,
                right: cw - 1,
                bottom: tab_h,
            };
            FillRect(mem_dc, &tab_sep, border_brush);
            let _ = DeleteObject(HGDIOBJ(border_brush.0));

            let tabs = [
                (SidebarTab::Outline, "大纲目录", 0, half_w),
                (SidebarTab::History, "文件历史", half_w, cw - 1),
            ];
            let accent_brush = CreateSolidBrush(self.palette.link.to_colorref());
            for (t_kind, label, left, right) in tabs {
                let is_active = self.tab == t_kind;
                let is_hover = self.hover == HoverTarget::Tab(t_kind);
                let old_font = SelectObject(
                    mem_dc,
                    HGDIOBJ(if is_active {
                        self.font_bold.0
                    } else {
                        self.font_normal.0
                    }),
                );
                let txt_color = if is_active {
                    self.palette.heading_text
                } else if is_hover {
                    self.palette.text
                } else {
                    self.palette.muted_text
                };
                SetTextColor(mem_dc, txt_color.to_colorref());
                let mut trc = RECT {
                    left,
                    top: 0,
                    right,
                    bottom: tab_h - 2,
                };
                let mut wide: Vec<u16> = label.encode_utf16().collect();
                let _ = DrawTextW(
                    mem_dc,
                    &mut wide,
                    &mut trc,
                    DT_SINGLELINE | DT_CENTER | DT_VCENTER | DT_NOPREFIX,
                );
                let _ = SelectObject(mem_dc, old_font);

                if is_active {
                    let ind_w = self.scale(36);
                    let ind_h = self.scale(2).max(2);
                    let cx = (left + right) / 2;
                    let irc = RECT {
                        left: cx - ind_w / 2,
                        top: tab_h - ind_h - 1,
                        right: cx + ind_w / 2,
                        bottom: tab_h - 1,
                    };
                    FillRect(mem_dc, &irc, accent_brush);
                }
            }

            // 3. 裁剪列表区域并绘制列表项
            let pad_x = self.scale(8);
            let top_start = tab_h + self.scale(6);
            let row_h = self.row_height_px();
            let hover_color = if self.palette.is_dark {
                bg_color.blend(RgbColor::new(255, 255, 255), 0.08)
            } else {
                bg_color.blend(RgbColor::new(0, 0, 0), 0.055)
            };
            let active_bg_color = bg_color.blend(self.palette.link, if self.palette.is_dark { 0.18 } else { 0.11 });

            let hover_brush = CreateSolidBrush(hover_color.to_colorref());
            let active_brush = CreateSolidBrush(active_bg_color.to_colorref());
            let null_pen = GetStockObject(NULL_PEN);

            if self.item_count() == 0 {
                let empty_msg = match self.tab {
                    SidebarTab::Outline => "暂无大纲标题",
                    SidebarTab::History => "近 7 天无打开记录",
                };
                let old_font = SelectObject(mem_dc, HGDIOBJ(self.font_normal.0));
                SetTextColor(mem_dc, self.palette.muted_text.to_colorref());
                let mut erc = RECT {
                    left: pad_x,
                    top: tab_h + self.scale(36),
                    right: cw - pad_x,
                    bottom: tab_h + self.scale(80),
                };
                let mut wide: Vec<u16> = empty_msg.encode_utf16().collect();
                let _ = DrawTextW(
                    mem_dc,
                    &mut wide,
                    &mut erc,
                    DT_SINGLELINE | DT_CENTER | DT_VCENTER | DT_NOPREFIX,
                );
                let _ = SelectObject(mem_dc, old_font);
            } else {
                let now_secs = current_epoch_secs();
                let radius = self.scale(6);

                for idx in 0..self.item_count() {
                    let y0 = top_start + (idx as i32) * row_h - self.scroll_y_px;
                    let y1 = y0 + row_h;
                    if y1 <= tab_h || y0 >= ch {
                        continue;
                    }
                    // 仅绘制在列表头下方的区域（若顶部轻微穿过 Tab 栏则稍后重绘 Tab 兜底，或裁剪）
                    let is_hovered = self.hover == HoverTarget::Row(idx);
                    let is_active = match self.tab {
                        SidebarTab::Outline => {
                            self.active_block_index == Some(self.outline[idx].block_index)
                        }
                        SidebarTab::History => self
                            .current_file_path
                            .as_deref()
                            .is_some_and(|cur| cur.eq_ignore_ascii_case(&self.history[idx].path)),
                    };

                    if is_active || is_hovered {
                        let old_pen = SelectObject(mem_dc, null_pen);
                        let old_br = SelectObject(
                            mem_dc,
                            HGDIOBJ(if is_active {
                                active_brush.0
                            } else {
                                hover_brush.0
                            }),
                        );
                        let _ = RoundRect(
                            mem_dc,
                            pad_x,
                            y0 + 1,
                            cw - pad_x - 2,
                            y1 - 1,
                            radius,
                            radius,
                        );
                        let _ = SelectObject(mem_dc, old_br);
                        let _ = SelectObject(mem_dc, old_pen);
                    }

                    if is_active {
                        let bar = RECT {
                            left: pad_x,
                            top: y0 + self.scale(6),
                            right: pad_x + self.scale(3).max(2),
                            bottom: y1 - self.scale(6),
                        };
                        FillRect(mem_dc, &bar, accent_brush);
                    }

                    match self.tab {
                        SidebarTab::Outline => {
                            let item = &self.outline[idx];
                            let indent = (item.level.saturating_sub(1).min(5) as i32)
                                * self.scale(11)
                                + self.scale(16);
                            let use_bold = item.level <= 2 || is_active;
                            let old_font = SelectObject(
                                mem_dc,
                                HGDIOBJ(if use_bold {
                                    self.font_bold.0
                                } else {
                                    self.font_normal.0
                                }),
                            );
                            let color = if is_active {
                                self.palette.link
                            } else if item.level == 1 {
                                self.palette.heading_text
                            } else {
                                self.palette.text
                            };
                            SetTextColor(mem_dc, color.to_colorref());
                            let mut rc = RECT {
                                left: indent,
                                top: y0,
                                right: cw - pad_x - self.scale(6),
                                bottom: y1,
                            };
                            let mut wide: Vec<u16> = item.title.encode_utf16().collect();
                            let _ = DrawTextW(
                                mem_dc,
                                &mut wide,
                                &mut rc,
                                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                            );
                            let _ = SelectObject(mem_dc, old_font);
                        }
                        SidebarTab::History => {
                            let entry = &self.history[idx];
                            let left_x = pad_x + self.scale(10);
                            let right_x = cw - pad_x - self.scale(6);

                            let old_font = SelectObject(
                                mem_dc,
                                HGDIOBJ(if is_active {
                                    self.font_bold.0
                                } else {
                                    self.font_normal.0
                                }),
                            );
                            SetTextColor(
                                mem_dc,
                                if is_active {
                                    self.palette.link
                                } else {
                                    self.palette.text
                                }
                                .to_colorref(),
                            );
                            let mut top_rc = RECT {
                                left: left_x,
                                top: y0 + self.scale(4),
                                right: right_x,
                                bottom: y0 + self.scale(24),
                            };
                            let mut name_w: Vec<u16> = entry.file_name().encode_utf16().collect();
                            let _ = DrawTextW(
                                mem_dc,
                                &mut name_w,
                                &mut top_rc,
                                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                            );

                            let _ = SelectObject(mem_dc, HGDIOBJ(self.font_small.0));
                            SetTextColor(mem_dc, self.palette.muted_text.to_colorref());
                            let subtitle = format_history_subtitle(entry, now_secs);
                            let mut sub_rc = RECT {
                                left: left_x,
                                top: y0 + self.scale(24),
                                right: right_x,
                                bottom: y1 - self.scale(3),
                            };
                            let mut sub_w: Vec<u16> = subtitle.encode_utf16().collect();
                            let _ = DrawTextW(
                                mem_dc,
                                &mut sub_w,
                                &mut sub_rc,
                                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                            );
                            let _ = SelectObject(mem_dc, old_font);
                        }
                    }
                }

                // 若有滚动行顶部跨越 Tab 栏边界，重盖一次顶部 Tab 栏区域保持干净裁剪
                if self.scroll_y_px > 0 {
                    let top_bg = CreateSolidBrush(bg_color.to_colorref());
                    let top_rc = RECT {
                        left: 0,
                        top: 0,
                        right: cw - 1,
                        bottom: tab_h,
                    };
                    FillRect(mem_dc, &top_rc, top_bg);
                    let _ = DeleteObject(HGDIOBJ(top_bg.0));

                    let sep_brush = CreateSolidBrush(self.palette.toolbar_border.to_colorref());
                    FillRect(mem_dc, &tab_sep, sep_brush);
                    let _ = DeleteObject(HGDIOBJ(sep_brush.0));

                    for (t_kind, label, left, right) in tabs {
                        let is_active = self.tab == t_kind;
                        let is_hover = self.hover == HoverTarget::Tab(t_kind);
                        let old_font = SelectObject(
                            mem_dc,
                            HGDIOBJ(if is_active {
                                self.font_bold.0
                            } else {
                                self.font_normal.0
                            }),
                        );
                        let txt_color = if is_active {
                            self.palette.heading_text
                        } else if is_hover {
                            self.palette.text
                        } else {
                            self.palette.muted_text
                        };
                        SetTextColor(mem_dc, txt_color.to_colorref());
                        let mut trc = RECT {
                            left,
                            top: 0,
                            right,
                            bottom: tab_h - 2,
                        };
                        let mut wide: Vec<u16> = label.encode_utf16().collect();
                        let _ = DrawTextW(
                            mem_dc,
                            &mut wide,
                            &mut trc,
                            DT_SINGLELINE | DT_CENTER | DT_VCENTER | DT_NOPREFIX,
                        );
                        let _ = SelectObject(mem_dc, old_font);
                        if is_active {
                            let ind_w = self.scale(36);
                            let ind_h = self.scale(2).max(2);
                            let cx = (left + right) / 2;
                            let irc = RECT {
                                left: cx - ind_w / 2,
                                top: tab_h - ind_h - 1,
                                right: cx + ind_w / 2,
                                bottom: tab_h - 1,
                            };
                            FillRect(mem_dc, &irc, accent_brush);
                        }
                    }
                }

                // 4. 极简细滚动条
                let list_h = (ch - tab_h).max(1);
                let total_h = self.content_height_px();
                if total_h > list_h {
                    let thumb_h = ((list_h as f32 / total_h as f32) * list_h as f32)
                        .round()
                        .max(self.scale(24) as f32) as i32;
                    let max_scroll = (total_h - list_h).max(1);
                    let thumb_y = tab_h
                        + ((self.scroll_y_px as f32 / max_scroll as f32)
                            * (list_h - thumb_h - 4).max(1) as f32)
                            .round() as i32
                        + 2;
                    let sb_color = bg_color.blend(self.palette.muted_text, 0.35);
                    let sb_pen = CreatePen(PS_SOLID, 1, sb_color.to_colorref());
                    let sb_brush = CreateSolidBrush(sb_color.to_colorref());
                    let old_p = SelectObject(mem_dc, HGDIOBJ(sb_pen.0));
                    let old_b = SelectObject(mem_dc, HGDIOBJ(sb_brush.0));
                    let _ = RoundRect(
                        mem_dc,
                        cw - self.scale(5),
                        thumb_y,
                        cw - 2,
                        thumb_y + thumb_h,
                        3,
                        3,
                    );
                    let _ = SelectObject(mem_dc, old_b);
                    let _ = SelectObject(mem_dc, old_p);
                    let _ = DeleteObject(HGDIOBJ(sb_brush.0));
                    let _ = DeleteObject(HGDIOBJ(sb_pen.0));
                }
            }

            let _ = DeleteObject(HGDIOBJ(hover_brush.0));
            let _ = DeleteObject(HGDIOBJ(active_brush.0));
            let _ = DeleteObject(HGDIOBJ(accent_brush.0));

            let _ = BitBlt(target_dc, 0, 0, cw, ch, Some(mem_dc), 0, 0, SRCCOPY);
            let _ = SelectObject(mem_dc, old_bmp);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            let _ = DeleteDC(mem_dc);
        }
    }
}

fn format_history_subtitle(entry: &HistoryEntry, now_secs: u64) -> String {
    let diff = now_secs.saturating_sub(entry.last_opened_secs);
    let rel = if diff < 60 {
        "刚刚".to_string()
    } else if diff < 3600 {
        format!("{} 分钟前", diff / 60)
    } else if diff < 86400 {
        format!("{} 小时前", diff / 3600)
    } else {
        format!("{} 天前", diff / 86400)
    };

    let parent_name = Path::new(&entry.path)
        .parent()
        .and_then(|p| p.file_name().or_else(|| p.as_os_str().is_empty().then_some(p.as_os_str())))
        .and_then(|s| s.to_str())
        .unwrap_or("");

    if parent_name.is_empty() {
        rel
    } else {
        format!("{rel} · {parent_name}")
    }
}

fn lparam_coords(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    (x, y)
}

unsafe extern "system" fn sidebar_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CREATE {
        let cs = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        }
        return LRESULT(0);
    }

    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut SidebarState;
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }

    if msg == WM_NCDESTROY {
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(ptr));
        }
        return LRESULT(0);
    }

    let state = unsafe { &mut *ptr };

    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_SIZE => {
            state.clamp_scroll();
            state.invalidate();
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            state.paint_to_dc(hdc);
            let _ = unsafe { EndPaint(hwnd, &ps) };
            LRESULT(0)
        }
        WM_PRINTCLIENT => {
            state.paint_to_dc(HDC(wparam.0 as *mut c_void));
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let cursor_id = if state.hover != HoverTarget::None {
                IDC_HAND
            } else {
                IDC_ARROW
            };
            if let Ok(cur) = unsafe { LoadCursorW(None, cursor_id) } {
                unsafe {
                    SetCursor(Some(cur));
                }
            }
            LRESULT(1)
        }
        WM_MOUSEMOVE => {
            if !state.tracking_leave {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                if unsafe { TrackMouseEvent(&mut tme) }.is_ok() {
                    state.tracking_leave = true;
                }
            }
            let (x, y) = lparam_coords(lparam);
            let next = state.hit_test(x, y);
            if next != state.hover {
                state.hover = next;
                state.invalidate();
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            state.tracking_leave = false;
            if state.hover != HoverTarget::None {
                state.hover = HoverTarget::None;
                state.invalidate();
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as i16;
            let step = state.scale(36);
            let old = state.scroll_y_px;
            state.scroll_y_px -= (delta as i32 * step) / 120;
            state.clamp_scroll();
            if state.scroll_y_px != old {
                state.invalidate();
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = lparam_coords(lparam);
            match state.hit_test(x, y) {
                HoverTarget::Tab(tab) => {
                    if state.tab != tab {
                        state.tab = tab;
                        state.scroll_y_px = 0;
                        state.hover = state.hit_test(x, y);
                        state.invalidate();
                    }
                }
                HoverTarget::Row(idx) => {
                    if let Ok(parent) = unsafe { GetParent(hwnd) }
                        && !parent.is_invalid()
                        && !parent.0.is_null()
                    {
                        match state.tab {
                            SidebarTab::Outline => {
                                if let Some(item) = state.outline.get(idx) {
                                    state.active_block_index = Some(item.block_index);
                                    state.invalidate();
                                    let _ = unsafe {
                                        PostMessageW(
                                            Some(parent),
                                            WM_APP_SIDEBAR_OUTLINE_JUMP,
                                            WPARAM(item.block_index),
                                            LPARAM(item.start_line as isize),
                                        )
                                    };
                                }
                            }
                            SidebarTab::History => {
                                if idx < state.history.len() {
                                    let _ = unsafe {
                                        PostMessageW(
                                            Some(parent),
                                            WM_APP_SIDEBAR_HISTORY_OPEN,
                                            WPARAM(idx),
                                            LPARAM(0),
                                        )
                                    };
                                }
                            }
                        }
                    }
                }
                HoverTarget::None => {}
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
