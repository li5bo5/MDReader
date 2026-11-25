//! MDReader 主窗口：Shazao 风格融合式自绘标题栏、左侧大纲/历史目录栏、二态同步滚动与交互闭环。
//!
//! 核心特性：
//! - 融合式极简自绘标题栏（`WM_NCCALCSIZE` + `WM_NCHITTEST` + `DwmExtendFrameIntoClientArea`），
//!   左上角依次排布：`[Logo]` · `[侧边栏折叠/展开矢量图标]` · 三个汉字按钮（阅读态：`打开 / 修改 / 关闭`，编辑态：`打开 / 保存 / 关闭`）；
//! - 左侧可收起/展开目录栏（顶部切换「大纲目录」与「文件历史」，历史自动保留 7 天）；
//! - 阅读模式 ↔ 修改模式切换时基于源码逻辑行号与块内插值精准双向同步滚动进度；
//! - `Esc` 快捷键确认逻辑：编辑态弹窗确认退回阅读模式，阅读态弹窗确认关闭软件。

use std::ffi::c_void;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{
    COLORREF, ERROR_SUCCESS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_USE_IMMERSIVE_DARK_MODE, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen,
    CreateSolidBrush, DT_CENTER, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteDC,
    DeleteObject, DrawTextW, EndPaint, FONT_CHARSET, FONT_CLIP_PRECISION, FONT_OUTPUT_PRECISION,
    FONT_QUALITY, FillRect, GetMonitorInfoW, GetStockObject, HBRUSH, HDC, HFONT, HGDIOBJ,
    HOLLOW_BRUSH, InvalidateRect, LineTo, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    MoveToEx, NULL_PEN, PAINTSTRUCT, PS_SOLID, RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME,
    RDW_INVALIDATE, Rectangle, RedrawWindow, RoundRect, SRCCOPY, ScreenToClient, SelectObject,
    SetBkColor, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Controls::{EM_SETSEL, MARGINS, WM_MOUSELEAVE};
use windows::Win32::UI::Controls::Dialogs::{
    OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
    OPENFILENAMEW,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT,
    TrackMouseEvent, VK_C, VK_CONTROL, VK_DOWN, VK_E, VK_END, VK_ESCAPE, VK_F, VK_F2, VK_F5,
    VK_HOME, VK_NEXT, VK_O, VK_PRIOR, VK_R, VK_RETURN, VK_S, VK_SHIFT, VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::HDROP;
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateIconFromResourceEx, CreateWindowExW, DI_NORMAL,
    DefWindowProcW, DestroyIcon, DestroyWindow, DispatchMessageW, DrawIconEx, EN_CHANGE,
    ES_AUTOHSCROLL, GWLP_USERDATA, GetClientRect, GetMessageW, GetSystemMetrics,
    GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, HICON, HMENU,
    HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT,
    HTTOPRIGHT, ICON_BIG, ICON_SMALL, IDC_ARROW, IDC_HAND, IDNO, IDOK, IDYES, IsIconic, IsZoomed,
    KillTimer, LR_DEFAULTCOLOR, LoadCursorW, MB_ICONERROR, MB_ICONQUESTION, MB_ICONWARNING, MB_OK,
    MB_OKCANCEL, MB_YESNOCANCEL, MSG, MessageBoxW, MoveWindow, NCCALCSIZE_PARAMS, PostMessageW,
    PostQuitMessage, RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, SW_MAXIMIZE, SW_MINIMIZE,
    SW_RESTORE, SW_SHOW, SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SendMessageW,
    SetCursor, SetTimer, SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow,
    TranslateMessage, WM_ACTIVATE, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_DESTROY,
    WM_DPICHANGED, WM_DROPFILES, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCALCSIZE, WM_NCHITTEST, WM_NCDESTROY, WM_NCMOUSEMOVE,
    WM_PAINT, WM_PRINTCLIENT, WM_SETCURSOR, WM_SETFONT, WM_SETICON, WM_SETTINGCHANGE, WM_SIZE,
    WM_TIMER, WNDCLASSW, WS_BORDER, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_ACCEPTFILES,
    WS_OVERLAPPEDWINDOW, WS_TABSTOP,
};
use windows::core::{HRESULT, PCWSTR, PWSTR, w};

use crate::core::config::{
    AppConfig, ConfigLoadResult, ConfigWatcher, load_config_with_source, resolve_config_path,
    save_window_geometry_if_valid,
};
use crate::core::history::{FileHistory, current_epoch_secs, resolve_history_path};
use crate::core::markdown::parse_markdown;
use crate::core::storage::{
    DocumentEncodingMeta, LoadedDocument, load_document, save_document_atomic,
};
use crate::ui::editor_view::EditorView;
use crate::ui::reader_view::ReaderView;
use crate::ui::sidebar::{SIDEBAR_WIDTH_DIP, SidebarView};
use crate::ui::theme::{RgbColor, ThemePalette};
use crate::ui::{
    APP_DISPLAY_NAME, AppMode, IDC_FIND_INPUT, IDC_FIND_STATUS, IDC_READER, WM_APP_READER_SCROLLED,
    WM_APP_SIDEBAR_HISTORY_OPEN, WM_APP_SIDEBAR_OUTLINE_JUMP,
};

/// 主窗口类名称。
const MAIN_CLASS_NAME: PCWSTR = w!("MDReaderMainWindow");

/// 融合式顶部自绘标题栏高度（逻辑 DIP）。
const TITLEBAR_HEIGHT_DIP: i32 = 40;

/// 页内查找栏高度（逻辑 DIP，仅 `Ctrl+F` 打开时展示）。
const FINDBAR_HEIGHT_DIP: i32 = 34;

/// 嵌入二进制的多尺寸扁平现代风应用图标（16/20/24/32/48/64/256 PNG-in-ICO）。
const APP_ICON_ICO: &[u8] = include_bytes!("../../assets/mdreader.ico");

/// 轻量级配置/文档热重载探测定时器 ID（仅在 `watchFileChanges = true` 时启用）。
const TIMER_WATCH_CHANGES: usize = 1;

/// 标题栏可交互自绘按钮枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TitleBtn {
    /// 左侧大纲/历史侧边栏折叠与展开图标按钮。
    SidebarToggle,
    /// 打开文件按钮（「打开」）。
    Open,
    /// 模式主按钮（阅读态「修改」/ 编辑态「保存」）。
    ModePrimary,
    /// 关闭软件按钮（「关闭」）。
    CloseApp,
    /// 右上角窗口最小化按钮。
    WinMin,
    /// 右上角窗口最大化/还原按钮。
    WinMax,
    /// 右上角窗口系统关闭按钮（`×`）。
    WinClose,
}

/// 从标准多帧 ICO 二进制中按目标像素尺寸挑选最匹配的 PNG 帧（优先 `w >= target_px` 中最小帧，否则取最大帧）。
fn select_ico_png_frame(ico: &[u8], target_px: i32) -> Option<&[u8]> {
    if ico.len() < 6 {
        return None;
    }
    let reserved = u16::from_le_bytes([ico[0], ico[1]]);
    let img_type = u16::from_le_bytes([ico[2], ico[3]]);
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    if reserved != 0 || img_type != 1 || count == 0 {
        return None;
    }
    let dir_end = 6usize.checked_add(count.checked_mul(16)?)?;
    if dir_end > ico.len() {
        return None;
    }

    let mut best_ge: Option<(i32, &[u8])> = None;
    let mut best_max: Option<(i32, &[u8])> = None;

    for i in 0..count {
        let entry = &ico[6 + i * 16..6 + (i + 1) * 16];
        let w = if entry[0] == 0 { 256 } else { i32::from(entry[0]) };
        let size = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
        let offset = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
        let Some(end) = offset.checked_add(size) else {
            continue;
        };
        if end > ico.len() || size < 8 {
            continue;
        }
        let slice = &ico[offset..end];
        if !slice.starts_with(b"\x89PNG\r\n\x1a\n") {
            continue;
        }
        if w >= target_px && best_ge.is_none_or(|(bw, _)| w < bw) {
            best_ge = Some((w, slice));
        }
        if best_max.is_none_or(|(bw, _)| w > bw) {
            best_max = Some((w, slice));
        }
    }

    best_ge.or(best_max).map(|(_, s)| s)
}

/// 从嵌入的 ICO PNG 帧创建透明抗锯齿 Win32 窗口图标句柄。
fn create_app_icon(size_px: i32) -> HICON {
    let clamped = size_px.max(16);
    let Some(png_slice) = select_ico_png_frame(APP_ICON_ICO, clamped) else {
        return HICON::default();
    };
    unsafe {
        CreateIconFromResourceEx(
            png_slice,
            true,
            0x0003_0000,
            clamped,
            clamped,
            LR_DEFAULTCOLOR,
        )
        .unwrap_or_default()
    }
}

/// 动态加载 `uxtheme.dll!SetWindowTheme` 用于 Windows 10/11 暗色原生控件外观美化（零硬依赖）。
fn set_control_dark_theme(hwnd: HWND, dark: bool) {
    type SetWindowThemeFn = unsafe extern "system" fn(HWND, PCWSTR, PCWSTR) -> HRESULT;
    unsafe {
        if let Ok(hmod) = LoadLibraryW(w!("uxtheme.dll"))
            && let Some(proc) = GetProcAddress(hmod, windows::core::s!("SetWindowTheme"))
        {
            let func: SetWindowThemeFn = std::mem::transmute(proc);
            let theme = if dark {
                w!("DarkMode_Explorer")
            } else {
                w!("Explorer")
            };
            let _ = func(hwnd, theme, PCWSTR::null());
        }
    }
}

/// 同步 Windows 11 沉浸式深浅色与 DWM 窗口边框投影。
fn apply_dwm_frame_and_dark_mode(hwnd: HWND, dark: bool) {
    let val: i32 = if dark { 1 } else { 0 };
    let margins = MARGINS {
        cxLeftWidth: 0,
        cxRightWidth: 0,
        cyTopHeight: 1,
        cyBottomHeight: 0,
    };
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&val as *const i32).cast::<c_void>(),
            std::mem::size_of::<i32>() as u32,
        );
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
    }
}

fn rect_contains(rc: &RECT, x: i32, y: i32) -> bool {
    x >= rc.left && x < rc.right && y >= rc.top && y < rc.bottom
}

/// 主窗口运行时完整状态。
struct MainWindowState {
    hwnd: HWND,
    instance: HINSTANCE,
    cli_config_override: Option<PathBuf>,
    config_target_path: PathBuf,
    config_load_result: ConfigLoadResult,
    config_watcher: ConfigWatcher,
    config: AppConfig,
    palette: ThemePalette,

    history_path: PathBuf,
    file_history: FileHistory,

    mode: AppMode,
    current_doc: Option<LoadedDocument>,
    doc_stamp: Option<crate::core::config::FileStamp>,
    dirty: bool,
    suppress_change_event: bool,
    find_bar_visible: bool,

    // 自绘标题栏交互状态
    hovered_btn: Option<TitleBtn>,
    pressed_btn: Option<TitleBtn>,
    tracking_mouse_leave: bool,

    // 查找栏控件与字体/图标资源
    txt_find: HWND,
    lbl_find_status: HWND,
    ui_font: HFONT,
    ui_font_bold: HFONT,
    cached_font_dpi: u32,
    icon_big: HICON,
    icon_small: HICON,
    icon_logo: HICON,
    cached_logo_px: i32,

    // GDI 主题背景画刷
    brush_toolbar: HBRUSH,
    brush_border: HBRUSH,
    brush_bg: HBRUSH,
    brush_input_bg: HBRUSH,

    // 侧边栏与双主视图
    sidebar: Option<SidebarView>,
    reader: Option<ReaderView>,
    editor: Option<EditorView>,
}

impl MainWindowState {
    fn dpi_u32(&self) -> u32 {
        let d = unsafe { GetDpiForWindow(self.hwnd) };
        if d > 0 { d } else { 96 }
    }

    fn dpi_scale(&self) -> f32 {
        self.dpi_u32() as f32 / 96.0
    }

    fn dip(&self, v: i32) -> i32 {
        (v as f32 * self.dpi_scale()).round() as i32
    }

    fn titlebar_height_px(&self) -> i32 {
        self.dip(TITLEBAR_HEIGHT_DIP)
    }

    fn findbar_height_px(&self) -> i32 {
        if self.find_bar_visible {
            self.dip(FINDBAR_HEIGHT_DIP)
        } else {
            0
        }
    }

    fn sidebar_width_px(&self) -> i32 {
        if self.sidebar.as_ref().is_some_and(SidebarView::is_visible) {
            self.dip(SIDEBAR_WIDTH_DIP)
        } else {
            0
        }
    }

    fn ensure_dpi_resources(&mut self) {
        let dpi = self.dpi_u32();
        if self.cached_font_dpi != dpi || self.ui_font.is_invalid() || self.ui_font.0.is_null() {
            unsafe {
                for f in [&mut self.ui_font, &mut self.ui_font_bold] {
                    if !f.is_invalid() && !f.0.is_null() {
                        let _ = DeleteObject(HGDIOBJ(f.0));
                        *f = HFONT::default();
                    }
                }
                let font_h = -self.dip(13);
                self.ui_font = CreateFontW(
                    font_h,
                    0,
                    0,
                    0,
                    400,
                    0,
                    0,
                    0,
                    FONT_CHARSET(1),
                    FONT_OUTPUT_PRECISION(0),
                    FONT_CLIP_PRECISION(0),
                    FONT_QUALITY(5),
                    0,
                    w!("Microsoft YaHei UI"),
                );
                self.ui_font_bold = CreateFontW(
                    font_h,
                    0,
                    0,
                    0,
                    600,
                    0,
                    0,
                    0,
                    FONT_CHARSET(1),
                    FONT_OUTPUT_PRECISION(0),
                    FONT_CLIP_PRECISION(0),
                    FONT_QUALITY(5),
                    0,
                    w!("Microsoft YaHei UI"),
                );
                for ctl in [self.txt_find, self.lbl_find_status] {
                    if !ctl.is_invalid() && !ctl.0.is_null() {
                        SendMessageW(
                            ctl,
                            WM_SETFONT,
                            Some(WPARAM(self.ui_font.0 as usize)),
                            Some(LPARAM(1)),
                        );
                    }
                }
            }
            self.cached_font_dpi = dpi;
        }

        let logo_px = self.dip(20).max(16);
        if self.cached_logo_px != logo_px || self.icon_logo.is_invalid() || self.icon_logo.0.is_null()
        {
            if !self.icon_logo.is_invalid() && !self.icon_logo.0.is_null() {
                let _ = unsafe { DestroyIcon(self.icon_logo) };
            }
            self.icon_logo = create_app_icon(logo_px);
            self.cached_logo_px = logo_px;
        }
    }

    fn rebuild_gdi_brushes(&mut self) {
        unsafe {
            for b in [
                self.brush_toolbar,
                self.brush_border,
                self.brush_bg,
                self.brush_input_bg,
            ] {
                if !b.is_invalid() && !b.0.is_null() {
                    let _ = DeleteObject(HGDIOBJ(b.0));
                }
            }
            self.brush_toolbar = CreateSolidBrush(self.palette.toolbar_bg.to_colorref());
            self.brush_border = CreateSolidBrush(self.palette.toolbar_border.to_colorref());
            self.brush_bg = CreateSolidBrush(self.palette.background.to_colorref());
            let input_bg = if self.palette.is_dark {
                RgbColor::new(0x18, 0x18, 0x1B)
            } else {
                RgbColor::new(0xFF, 0xFF, 0xFF)
            };
            self.brush_input_bg = CreateSolidBrush(input_bg.to_colorref());
        }
    }

    /// 计算融合标题栏上各个自绘按钮在客户区中的矩形坐标。
    fn titlebar_button_rect(&self, btn: TitleBtn, client_w: i32) -> RECT {
        let tb_h = self.titlebar_height_px();
        let btn_y = self.dip(6);
        let btn_h = tb_h - btn_y * 2;
        let sys_w = self.dip(46);

        match btn {
            TitleBtn::SidebarToggle => {
                let x = self.dip(38);
                RECT {
                    left: x,
                    top: btn_y,
                    right: x + self.dip(30),
                    bottom: btn_y + btn_h,
                }
            }
            TitleBtn::Open => {
                let x = self.dip(78);
                RECT {
                    left: x,
                    top: btn_y,
                    right: x + self.dip(52),
                    bottom: btn_y + btn_h,
                }
            }
            TitleBtn::ModePrimary => {
                let x = self.dip(134);
                RECT {
                    left: x,
                    top: btn_y,
                    right: x + self.dip(52),
                    bottom: btn_y + btn_h,
                }
            }
            TitleBtn::CloseApp => {
                let x = self.dip(190);
                RECT {
                    left: x,
                    top: btn_y,
                    right: x + self.dip(52),
                    bottom: btn_y + btn_h,
                }
            }
            TitleBtn::WinMin => RECT {
                left: (client_w - sys_w * 3).max(self.dip(250)),
                top: 0,
                right: (client_w - sys_w * 2).max(self.dip(250)),
                bottom: tb_h,
            },
            TitleBtn::WinMax => RECT {
                left: (client_w - sys_w * 2).max(self.dip(250)),
                top: 0,
                right: (client_w - sys_w).max(self.dip(250)),
                bottom: tb_h,
            },
            TitleBtn::WinClose => RECT {
                left: (client_w - sys_w).max(self.dip(250)),
                top: 0,
                right: client_w,
                bottom: tb_h,
            },
        }
    }

    fn hit_test_titlebar_btn(&self, x: i32, y: i32, client_w: i32) -> Option<TitleBtn> {
        if y < 0 || y >= self.titlebar_height_px() {
            return None;
        }
        let buttons = [
            TitleBtn::SidebarToggle,
            TitleBtn::Open,
            TitleBtn::ModePrimary,
            TitleBtn::CloseApp,
            TitleBtn::WinMin,
            TitleBtn::WinMax,
            TitleBtn::WinClose,
        ];
        for b in buttons {
            let rc = self.titlebar_button_rect(b, client_w);
            if rc.right > rc.left && rect_contains(&rc, x, y) {
                return Some(b);
            }
        }
        None
    }

    fn invalidate_titlebar(&self) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
            rc.bottom = self.titlebar_height_px() + self.findbar_height_px();
            let _ = InvalidateRect(Some(self.hwnd), Some(&rc), false);
        }
    }

    fn init_children(&mut self) -> windows::core::Result<()> {
        self.ensure_dpi_resources();

        // 页内查找栏控件（初始隐藏，仅 Ctrl+F 显示）
        let edit_style = WS_CHILD
            | WS_BORDER
            | WS_TABSTOP
            | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(ES_AUTOHSCROLL as u32);
        self.txt_find = unsafe {
            CreateWindowExW(
                Default::default(),
                w!("EDIT"),
                w!(""),
                edit_style,
                8,
                44,
                240,
                24,
                Some(self.hwnd),
                Some(HMENU(IDC_FIND_INPUT as usize as *mut c_void)),
                Some(self.instance),
                None,
            )?
        };

        self.lbl_find_status = unsafe {
            CreateWindowExW(
                Default::default(),
                w!("STATIC"),
                w!(""),
                WS_CHILD,
                258,
                48,
                160,
                20,
                Some(self.hwnd),
                Some(HMENU(IDC_FIND_STATUS as usize as *mut c_void)),
                Some(self.instance),
                None,
            )?
        };

        for ctl in [self.txt_find, self.lbl_find_status] {
            unsafe {
                SendMessageW(
                    ctl,
                    WM_SETFONT,
                    Some(WPARAM(self.ui_font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
        }

        // 创建左侧大纲/历史目录栏（默认展开并停留在「大纲」Tab）
        let mut sidebar = SidebarView::create(self.hwnd, self.instance, self.palette)?;
        sidebar.set_history(self.file_history.entries.clone(), None);
        self.sidebar = Some(sidebar);

        // 创建 Direct2D 阅读画布；Msftedit 编辑区按需懒创建以压低冷启动内存
        let reader = ReaderView::create(
            self.hwnd,
            self.instance,
            IDC_READER,
            &self.config,
            self.palette,
        )?;
        self.reader = Some(reader);
        self.editor = None;

        self.apply_theme_to_chrome();
        self.sync_watch_timer();
        self.layout_children();
        self.update_window_title();
        Ok(())
    }

    fn apply_theme_to_chrome(&mut self) {
        self.rebuild_gdi_brushes();
        apply_dwm_frame_and_dark_mode(self.hwnd, self.palette.is_dark);
        set_control_dark_theme(self.txt_find, self.palette.is_dark);
        if let Some(ed) = &self.editor {
            set_control_dark_theme(ed.hwnd(), self.palette.is_dark);
        }
        if let Some(sb) = &mut self.sidebar {
            sb.apply_palette(self.palette);
        }
        unsafe {
            let _ = RedrawWindow(
                Some(self.hwnd),
                None,
                None,
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_FRAME,
            );
        }
    }

    fn sync_watch_timer(&self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_WATCH_CHANGES);
            if self.config.watch_file_changes {
                let _ = SetTimer(Some(self.hwnd), TIMER_WATCH_CHANGES, 1000, None);
            }
        }
    }

    /// 布局融合标题栏下方的左侧目录栏、可选查找栏以及右侧阅读/编辑主区域。
    fn layout_children(&mut self) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        let cw = (rc.right - rc.left).max(1);
        let ch = (rc.bottom - rc.top).max(1);

        let tb_h = self.titlebar_height_px();
        let body_h = (ch - tb_h).max(1);

        let sb_w = if cw > self.dip(420) {
            self.sidebar_width_px()
        } else {
            self.sidebar_width_px().min(cw / 2)
        };

        if let Some(sb) = &self.sidebar
            && sb.is_visible()
        {
            sb.set_bounds(0, tb_h, sb_w, body_h);
        }

        let main_x = sb_w;
        let main_w = (cw - main_x).max(1);

        let fb_h = self.findbar_height_px();
        if self.find_bar_visible {
            let pad_x = self.dip(12);
            let gap = self.dip(8);
            let input_h = self.dip(24);
            let input_y = tb_h + (fb_h - input_h) / 2;
            let input_w = self.dip(250).min((main_w - pad_x * 2 - 90).max(100));
            unsafe {
                let _ = MoveWindow(
                    self.txt_find,
                    main_x + pad_x,
                    input_y,
                    input_w,
                    input_h,
                    true,
                );
                let _ = MoveWindow(
                    self.lbl_find_status,
                    main_x + pad_x + input_w + gap,
                    input_y + self.dip(3),
                    self.dip(140),
                    input_h - self.dip(4),
                    true,
                );
            }
        }

        let content_top = tb_h + fb_h;
        let content_h = (ch - content_top).max(1);

        if let Some(reader) = &self.reader {
            unsafe {
                let _ = MoveWindow(reader.hwnd(), main_x, content_top, main_w, content_h, true);
            }
        }
        if let Some(editor) = &mut self.editor {
            editor.set_bounds(main_x, content_top, main_w, content_h);
        }
    }

    fn current_display_filename(&self) -> String {
        let filename = self
            .current_doc
            .as_ref()
            .and_then(|d| d.path.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "未命名".to_string());
        if self.mode == AppMode::Editing && self.dirty {
            format!("{filename} *")
        } else {
            filename
        }
    }

    /// 格式化并设置系统任务栏窗口标题：`"<filename>[*] - <模式> - MDReader"`。
    fn update_window_title(&self) {
        let filename = self
            .current_doc
            .as_ref()
            .and_then(|d| d.path.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "未命名".to_string());
        let dirty_mark = if self.mode == AppMode::Editing && self.dirty {
            "*"
        } else {
            ""
        };
        let title = format!(
            "{filename}{dirty_mark} - {} - {APP_DISPLAY_NAME}",
            self.mode.title_label()
        );
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let _ = SetWindowTextW(self.hwnd, PCWSTR(wide.as_ptr()));
        }
        self.invalidate_titlebar();
    }

    /// 记录文件历史（自动保留 7 天并原子写盘），同时刷新侧边栏历史列表。
    fn record_file_history(&mut self, path: &Path) {
        let now = current_epoch_secs();
        self.file_history.record_open(path, now);
        let _ = self.file_history.save(&self.history_path);
        if let Some(sb) = &mut self.sidebar {
            sb.set_history(self.file_history.entries.clone(), Some(path));
        }
    }

    /// 同步阅读画布当前所在章节标题到左侧大纲高亮。
    fn sync_sidebar_active_heading(&mut self) {
        let active = self
            .reader
            .as_mut()
            .and_then(ReaderView::active_heading_block);
        if let Some(sb) = &mut self.sidebar {
            sb.set_active_heading(active);
        }
    }

    /// 载入指定磁盘路径的 Markdown 文件并切入阅读模式；路径异常时优雅展示错误横幅不崩溃。
    fn open_file(&mut self, path: &Path) {
        match load_document(path) {
            Ok(doc) => {
                let opened_path = doc.path.clone();
                let doc_dir = opened_path.parent().map(Path::to_path_buf);
                let parsed = parse_markdown(&doc.text);
                let outline = parsed.outline();
                self.doc_stamp = ConfigWatcher::probe_stamp(&opened_path);
                self.current_doc = Some(doc);
                self.dirty = false;
                if let Some(reader) = &mut self.reader {
                    reader.set_document(parsed, doc_dir, false, true);
                }
                let active_heading = self
                    .reader
                    .as_mut()
                    .and_then(ReaderView::active_heading_block);
                if let Some(sb) = &mut self.sidebar {
                    sb.set_outline(outline, active_heading);
                }
                self.record_file_history(&opened_path);
                self.switch_to_reading_immediate(None);
            }
            Err(err) => {
                self.current_doc = Some(LoadedDocument {
                    path: path.to_path_buf(),
                    text: String::new(),
                    meta: DocumentEncodingMeta::default(),
                });
                self.doc_stamp = None;
                self.dirty = false;
                if let Some(reader) = &mut self.reader {
                    reader.set_error_banner(format!(
                        "无法打开目标文件：{}\n原因：{err}",
                        path.display()
                    ));
                }
                if let Some(sb) = &mut self.sidebar {
                    sb.set_outline(Vec::new(), None);
                    sb.set_history(self.file_history.entries.clone(), None);
                }
                self.switch_to_reading_immediate(None);
            }
        }
    }

    /// 立即切换回阅读模式；若传入 `sync_source_line` 则同步滚动到对应源码逻辑行。
    fn switch_to_reading_immediate(&mut self, sync_source_line: Option<usize>) {
        self.mode = AppMode::Reading;
        self.dirty = false;

        if let Some(editor) = &self.editor {
            editor.set_visible(false);
        }
        self.layout_children();

        if let Some(reader) = &mut self.reader {
            if let Some(line) = sync_source_line {
                reader.scroll_to_source_line(line);
            }
            unsafe {
                let _ = ShowWindow(reader.hwnd(), SW_SHOW);
                let _ = SetFocus(Some(reader.hwnd()));
            }
            reader.invalidate();
        }

        self.sync_sidebar_active_heading();
        self.update_window_title();
    }

    /// 切换进入修改模式，并将当前阅读进度精准同步到编辑区对应源码行。
    fn enter_editing_mode(&mut self) {
        if self.mode == AppMode::Editing {
            return;
        }
        if self.reader.as_ref().is_some_and(ReaderView::has_error_banner) {
            return;
        }

        let sync_line = self
            .reader
            .as_mut()
            .map(ReaderView::current_source_line)
            .unwrap_or(0);
        let initial_text = self
            .current_doc
            .as_ref()
            .map(|d| d.text.clone())
            .unwrap_or_default();

        self.close_find_bar();
        self.mode = AppMode::Editing;
        self.dirty = false;

        if let Some(reader) = &self.reader {
            unsafe {
                let _ = ShowWindow(reader.hwnd(), SW_HIDE);
            }
        }

        // 按需懒加载创建 Msftedit.dll 编辑区，保持阅读冷启动零额外 DLL 内存开销
        if self.editor.is_none()
            && let Ok(ed) = EditorView::create(self.hwnd, self.instance, &self.config, self.palette)
        {
            set_control_dark_theme(ed.hwnd(), self.palette.is_dark);
            self.editor = Some(ed);
        }

        self.layout_children();

        self.suppress_change_event = true;
        if let Some(editor) = &self.editor {
            editor.load_text(&initial_text);
            editor.set_visible(true);
            editor.scroll_to_source_line(sync_line);
        }
        self.suppress_change_event = false;
        self.dirty = false;

        self.update_window_title();
    }

    /// 原子保存当前修改内容并自动切回阅读模式，同时将编辑区滚动进度同步回阅读区。
    fn save_and_switch_to_reading(&mut self) -> bool {
        if self.mode != AppMode::Editing {
            return true;
        }

        let (new_text, sync_line) = match &self.editor {
            Some(ed) => (ed.get_text_lf(), ed.first_visible_source_line()),
            None => return false,
        };

        let (target_path, meta, is_existing_doc) = if let Some(doc) = &self.current_doc {
            (doc.path.clone(), doc.meta, true)
        } else {
            let Some(chosen) = show_save_file_dialog(self.hwnd) else {
                return false;
            };
            (chosen, DocumentEncodingMeta::default(), false)
        };

        if is_existing_doc && !target_path.is_file() {
            let msg = format!(
                "目标原文件已不存在或已被外部删除，您的修改已保留在编辑区中：\n\n路径：{}",
                target_path.display()
            );
            show_message_box(self.hwnd, "保存错误", &msg, MB_OK | MB_ICONERROR);
            if let Some(ed) = &self.editor {
                let _ = unsafe { SetFocus(Some(ed.hwnd())) };
            }
            return false;
        }

        match save_document_atomic(&target_path, &new_text, &meta) {
            Ok(()) => {
                let doc_dir = target_path.parent().map(Path::to_path_buf);
                let parsed = parse_markdown(&new_text);
                let outline = parsed.outline();
                self.doc_stamp = ConfigWatcher::probe_stamp(&target_path);
                self.current_doc = Some(LoadedDocument {
                    path: target_path.clone(),
                    text: new_text,
                    meta,
                });
                self.dirty = false;
                if let Some(reader) = &mut self.reader {
                    reader.set_document(parsed, doc_dir, false, false);
                }
                if let Some(sb) = &mut self.sidebar {
                    sb.set_outline(outline, None);
                }
                self.record_file_history(&target_path);
                self.switch_to_reading_immediate(Some(sync_line));
                true
            }
            Err(err) => {
                let msg = format!(
                    "保存文件失败，您的修改已保留在编辑区中：\n\n路径：{}\n错误：{err}",
                    target_path.display()
                );
                show_message_box(self.hwnd, "保存错误", &msg, MB_OK | MB_ICONERROR);
                if let Some(ed) = &self.editor {
                    let _ = unsafe { SetFocus(Some(ed.hwnd())) };
                }
                false
            }
        }
    }

    /// 脏状态确认守卫：返回 `true` 表示允许继续离开，`false` 表示取消离开。
    fn confirm_discard_or_save_if_dirty(&mut self) -> bool {
        if self.mode != AppMode::Editing || !self.dirty {
            return true;
        }
        let ans = show_message_box(
            self.hwnd,
            "未保存确认",
            "当前修改尚未保存，是否保存更改？",
            MB_YESNOCANCEL | MB_ICONWARNING,
        );
        if ans == IDYES {
            self.save_and_switch_to_reading()
        } else if ans == IDNO {
            self.dirty = false;
            true
        } else {
            false
        }
    }

    /// 编辑模式下按 `Esc` 或 `Ctrl+E`/`F2` 退出编辑返回阅读模式（均需弹窗确认并同步滚动位置）。
    fn handle_exit_edit_with_confirm(&mut self, require_confirm_when_clean: bool) {
        if self.mode != AppMode::Editing {
            return;
        }
        let sync_line = self
            .editor
            .as_ref()
            .map(EditorView::first_visible_source_line);

        if !self.dirty {
            if require_confirm_when_clean {
                let ans = show_message_box(
                    self.hwnd,
                    "返回阅读模式",
                    "确定退出修改模式并返回阅读模式吗？",
                    MB_OKCANCEL | MB_ICONQUESTION,
                );
                if ans != IDOK {
                    if let Some(ed) = &self.editor {
                        let _ = unsafe { SetFocus(Some(ed.hwnd())) };
                    }
                    return;
                }
            }
            self.switch_to_reading_immediate(sync_line);
            return;
        }

        let ans = show_message_box(
            self.hwnd,
            "未保存确认",
            "当前修改尚未保存，是否保存更改并返回阅读模式？",
            MB_YESNOCANCEL | MB_ICONWARNING,
        );
        if ans == IDYES {
            let _ = self.save_and_switch_to_reading();
        } else if ans == IDNO {
            self.switch_to_reading_immediate(sync_line);
        } else if let Some(ed) = &self.editor {
            let _ = unsafe { SetFocus(Some(ed.hwnd())) };
        }
    }

    /// 阅读模式下按 `Esc` 弹窗确认关闭软件。
    fn handle_escape_close_app_confirm(&self) {
        let ans = show_message_box(
            self.hwnd,
            "退出软件确认",
            "确定关闭 MDReader 吗？",
            MB_OKCANCEL | MB_ICONQUESTION,
        );
        if ans == IDOK {
            let _ = unsafe { PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
    }

    fn handle_open_file_dialog(&mut self) {
        if !self.confirm_discard_or_save_if_dirty() {
            return;
        }
        if let Some(path) = show_open_file_dialog(self.hwnd) {
            self.open_file(&path);
        }
    }

    fn activate_titlebar_button(&mut self, btn: TitleBtn) {
        match btn {
            TitleBtn::SidebarToggle => {
                if let Some(sb) = &mut self.sidebar {
                    sb.toggle_visible();
                }
                self.layout_children();
                self.invalidate_titlebar();
            }
            TitleBtn::Open => {
                self.handle_open_file_dialog();
            }
            TitleBtn::ModePrimary => {
                if self.mode == AppMode::Reading {
                    self.enter_editing_mode();
                } else {
                    let _ = self.save_and_switch_to_reading();
                }
            }
            TitleBtn::CloseApp | TitleBtn::WinClose => {
                let _ = unsafe { PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
            }
            TitleBtn::WinMin => unsafe {
                let _ = ShowWindow(self.hwnd, SW_MINIMIZE);
            },
            TitleBtn::WinMax => unsafe {
                if IsZoomed(self.hwnd).as_bool() {
                    let _ = ShowWindow(self.hwnd, SW_RESTORE);
                } else {
                    let _ = ShowWindow(self.hwnd, SW_MAXIMIZE);
                }
            },
        }
    }

    fn open_find_bar(&mut self) {
        if self.mode != AppMode::Reading {
            return;
        }
        if !self.find_bar_visible {
            self.find_bar_visible = true;
            unsafe {
                let _ = ShowWindow(self.txt_find, SW_SHOW);
                let _ = ShowWindow(self.lbl_find_status, SW_SHOW);
            }
            self.layout_children();
        }
        unsafe {
            let _ = SetFocus(Some(self.txt_find));
            SendMessageW(self.txt_find, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
        self.invalidate_titlebar();
        self.on_find_text_changed();
    }

    fn close_find_bar(&mut self) {
        if !self.find_bar_visible {
            return;
        }
        self.find_bar_visible = false;
        unsafe {
            let _ = ShowWindow(self.txt_find, SW_HIDE);
            let _ = ShowWindow(self.lbl_find_status, SW_HIDE);
            let _ = SetWindowTextW(self.lbl_find_status, w!(""));
        }
        if let Some(reader) = &mut self.reader {
            reader.clear_find();
            if self.mode == AppMode::Reading {
                let _ = unsafe { SetFocus(Some(reader.hwnd())) };
            }
        }
        self.layout_children();
        self.invalidate_titlebar();
    }

    fn on_find_text_changed(&mut self) {
        let len = unsafe { GetWindowTextLengthW(self.txt_find) };
        let query = if len > 0 {
            let mut buf = vec![0u16; len as usize + 2];
            let n = unsafe { GetWindowTextW(self.txt_find, &mut buf) };
            String::from_utf16_lossy(&buf[..n.max(0) as usize])
        } else {
            String::new()
        };

        let status = if let Some(reader) = &mut self.reader {
            reader.set_find_query(&query)
        } else {
            String::new()
        };
        let wide: Vec<u16> = status.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let _ = SetWindowTextW(self.lbl_find_status, PCWSTR(wide.as_ptr()));
        }
    }

    fn step_find(&mut self, forward: bool) {
        if !self.find_bar_visible {
            return;
        }
        let status = if let Some(reader) = &mut self.reader {
            reader.step_find_match(forward)
        } else {
            String::new()
        };
        let wide: Vec<u16> = status.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let _ = SetWindowTextW(self.lbl_find_status, PCWSTR(wide.as_ptr()));
        }
    }

    fn reload_config(&mut self, force: bool) {
        let next_res = if force {
            let is_cli = self.cli_config_override.is_some();
            let _ = self.config_watcher.force_reload();
            load_config_with_source(Some(&self.config_target_path), is_cli)
        } else if let Some(res) = self.config_watcher.reload_if_changed() {
            res
        } else {
            return;
        };

        self.config = next_res.config.clone();
        self.config_load_result = next_res;
        self.palette = ThemePalette::from_config(&self.config);

        if let Some(reader) = &mut self.reader {
            reader.apply_config(&self.config, self.palette);
        }
        if let Some(editor) = &mut self.editor {
            editor.apply_config(&self.config, self.palette);
        }
        self.apply_theme_to_chrome();
        self.sync_watch_timer();
    }

    fn poll_external_changes(&mut self) {
        self.reload_config(false);
        if self.config.watch_file_changes
            && self.mode == AppMode::Reading
            && let Some(doc) = &self.current_doc
        {
            let cur_stamp = ConfigWatcher::probe_stamp(&doc.path);
            if cur_stamp.is_some() && cur_stamp != self.doc_stamp {
                let p = doc.path.clone();
                if let Ok(reloaded) = load_document(&p) {
                    let doc_dir = reloaded.path.parent().map(Path::to_path_buf);
                    let parsed = parse_markdown(&reloaded.text);
                    let outline = parsed.outline();
                    self.doc_stamp = cur_stamp;
                    self.current_doc = Some(reloaded);
                    if let Some(reader) = &mut self.reader {
                        reader.set_document(parsed, doc_dir, false, false);
                    }
                    let active_heading = self
                        .reader
                        .as_mut()
                        .and_then(ReaderView::active_heading_block);
                    if let Some(sb) = &mut self.sidebar {
                        sb.set_outline(outline, active_heading);
                    }
                }
            }
        }
    }

    fn on_system_theme_changed(&mut self) {
        if self.config.theme == crate::core::config::ThemeMode::System {
            let new_pal = ThemePalette::from_config(&self.config);
            if new_pal != self.palette {
                self.palette = new_pal;
                if let Some(reader) = &mut self.reader {
                    reader.apply_config(&self.config, self.palette);
                }
                if let Some(editor) = &mut self.editor {
                    editor.apply_config(&self.config, self.palette);
                }
                self.apply_theme_to_chrome();
            }
        }
    }

    fn persist_window_geometry_if_safe(&self) {
        if unsafe { IsIconic(self.hwnd).as_bool() || IsZoomed(self.hwnd).as_bool() } {
            return;
        }
        let mut wr = RECT::default();
        if unsafe { GetWindowRect(self.hwnd, &mut wr) }.is_err() {
            return;
        }
        let w = (wr.right - wr.left).max(0) as u32;
        let h = (wr.bottom - wr.top).max(0) as u32;
        let _ = save_window_geometry_if_valid(
            &self.config_load_result,
            &self.config_target_path,
            w,
            h,
        );
    }

    /// 双缓冲绘制融合式自绘标题栏（Logo + 侧边栏折叠图标 + 3 个汉字模式按钮 + 文档标题 + 右侧系统控制按钮）。
    fn paint_chrome(&mut self, target_dc: HDC) {
        self.ensure_dpi_resources();
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        let cw = (rc.right - rc.left).max(1);
        let ch = (rc.bottom - rc.top).max(1);
        let tb_h = self.titlebar_height_px();
        let fb_h = self.findbar_height_px();
        let chrome_bot = (tb_h + fb_h).min(ch);

        unsafe {
            let mem_dc = CreateCompatibleDC(Some(target_dc));
            if !mem_dc.is_invalid() && !mem_dc.0.is_null() {
                let bmp = CreateCompatibleBitmap(target_dc, cw, chrome_bot.max(1));
                if !bmp.is_invalid() && !bmp.0.is_null() {
                    let old_bmp = SelectObject(mem_dc, HGDIOBJ(bmp.0));
                    let _ = SetBkMode(mem_dc, TRANSPARENT);

                    // 1. 标题栏背景与底部分隔细线
                    let full_tb = RECT {
                        left: 0,
                        top: 0,
                        right: cw,
                        bottom: chrome_bot,
                    };
                    FillRect(mem_dc, &full_tb, self.brush_toolbar);

                    let tb_sep = RECT {
                        left: 0,
                        top: (tb_h - 1).max(0),
                        right: cw,
                        bottom: tb_h,
                    };
                    FillRect(mem_dc, &tb_sep, self.brush_border);
                    if fb_h > 0 {
                        let fb_sep = RECT {
                            left: 0,
                            top: (chrome_bot - 1).max(0),
                            right: cw,
                            bottom: chrome_bot,
                        };
                        FillRect(mem_dc, &fb_sep, self.brush_border);
                    }

                    // 2. 左上角 Logo 图标
                    let logo_px = self.dip(20).max(16);
                    let logo_x = self.dip(12);
                    let logo_y = (tb_h - logo_px) / 2;
                    if !self.icon_logo.is_invalid() && !self.icon_logo.0.is_null() {
                        let _ = DrawIconEx(
                            mem_dc,
                            logo_x,
                            logo_y,
                            self.icon_logo,
                            logo_px,
                            logo_px,
                            0,
                            None,
                            DI_NORMAL,
                        );
                    }

                    // 3. 通用圆角悬停画刷准备
                    let hover_bg = if self.palette.is_dark {
                        self.palette.toolbar_bg.blend(RgbColor::new(255, 255, 255), 0.10)
                    } else {
                        self.palette.toolbar_bg.blend(RgbColor::new(0, 0, 0), 0.065)
                    };
                    let press_bg = if self.palette.is_dark {
                        self.palette.toolbar_bg.blend(RgbColor::new(255, 255, 255), 0.16)
                    } else {
                        self.palette.toolbar_bg.blend(RgbColor::new(0, 0, 0), 0.11)
                    };
                    let hover_brush = CreateSolidBrush(hover_bg.to_colorref());
                    let press_brush = CreateSolidBrush(press_bg.to_colorref());
                    let null_pen = GetStockObject(NULL_PEN);
                    let hollow_brush = GetStockObject(HOLLOW_BRUSH);
                    let radius = self.dip(6);

                    // 4. 绘制紧贴 Logo 的「侧边栏折叠/展开矢量图标按钮」（无汉字，还原参考图）
                    let sb_rc = self.titlebar_button_rect(TitleBtn::SidebarToggle, cw);
                    let sb_open = self.sidebar.as_ref().is_some_and(SidebarView::is_visible);
                    if self.hovered_btn == Some(TitleBtn::SidebarToggle)
                        || self.pressed_btn == Some(TitleBtn::SidebarToggle)
                    {
                        let old_p = SelectObject(mem_dc, null_pen);
                        let old_b = SelectObject(
                            mem_dc,
                            HGDIOBJ(
                                if self.pressed_btn == Some(TitleBtn::SidebarToggle) {
                                    press_brush.0
                                } else {
                                    hover_brush.0
                                },
                            ),
                        );
                        let _ = RoundRect(
                            mem_dc,
                            sb_rc.left,
                            sb_rc.top,
                            sb_rc.right,
                            sb_rc.bottom,
                            radius,
                            radius,
                        );
                        let _ = SelectObject(mem_dc, old_b);
                        let _ = SelectObject(mem_dc, old_p);
                    }
                    self.draw_sidebar_toggle_icon(mem_dc, &sb_rc, sb_open);

                    // 分隔竖线（在侧边栏图标按钮与 3 个汉字按钮之间）
                    let vsep_x = self.dip(73);
                    let vsep = RECT {
                        left: vsep_x,
                        top: self.dip(12),
                        right: vsep_x + 1,
                        bottom: tb_h - self.dip(12),
                    };
                    FillRect(mem_dc, &vsep, self.brush_border);

                    // 5. 绘制三个汉字按钮：阅读态（打开 / 修改 / 关闭），编辑态（打开 / 保存 / 关闭）
                    let primary_label = match self.mode {
                        AppMode::Reading => "修改",
                        AppMode::Editing => "保存",
                    };
                    let text_buttons = [
                        (TitleBtn::Open, "打开", false),
                        (TitleBtn::ModePrimary, primary_label, self.mode == AppMode::Editing),
                        (TitleBtn::CloseApp, "关闭", false),
                    ];
                    for (btn_kind, label, emphasize) in text_buttons {
                        let brc = self.titlebar_button_rect(btn_kind, cw);
                        let is_hov = self.hovered_btn == Some(btn_kind);
                        let is_prs = self.pressed_btn == Some(btn_kind);

                        if is_hov || is_prs || (emphasize && self.dirty) {
                            let custom_accent_bg = self.palette.toolbar_bg.blend(
                                self.palette.link,
                                if self.palette.is_dark { 0.22 } else { 0.12 },
                            );
                            let accent_br = CreateSolidBrush(custom_accent_bg.to_colorref());
                            let old_p = SelectObject(mem_dc, null_pen);
                            let chosen_br = if is_prs {
                                press_brush.0
                            } else if is_hov {
                                hover_brush.0
                            } else {
                                accent_br.0
                            };
                            let old_b = SelectObject(mem_dc, HGDIOBJ(chosen_br));
                            let _ = RoundRect(
                                mem_dc,
                                brc.left,
                                brc.top,
                                brc.right,
                                brc.bottom,
                                radius,
                                radius,
                            );
                            let _ = SelectObject(mem_dc, old_b);
                            let _ = SelectObject(mem_dc, old_p);
                            let _ = DeleteObject(HGDIOBJ(accent_br.0));
                        }

                        let old_f = SelectObject(
                            mem_dc,
                            HGDIOBJ(if emphasize {
                                self.ui_font_bold.0
                            } else {
                                self.ui_font.0
                            }),
                        );
                        let txt_col = if emphasize {
                            self.palette.link
                        } else {
                            self.palette.text
                        };
                        SetTextColor(mem_dc, txt_col.to_colorref());
                        let mut trc = brc;
                        let mut wide: Vec<u16> = label.encode_utf16().collect();
                        let _ = DrawTextW(
                            mem_dc,
                            &mut wide,
                            &mut trc,
                            DT_SINGLELINE | DT_CENTER | DT_VCENTER | DT_NOPREFIX,
                        );
                        let _ = SelectObject(mem_dc, old_f);
                    }

                    // 6. 中间区域柔和展示当前文档文件名（窗口过窄时自动收缩避让）
                    let title_left = self.dip(252);
                    let title_right = (cw - self.dip(46) * 3 - self.dip(12)).max(title_left);
                    if title_right - title_left >= self.dip(80) && self.current_doc.is_some() {
                        let doc_title = self.current_display_filename();
                        let old_f = SelectObject(mem_dc, HGDIOBJ(self.ui_font.0));
                        SetTextColor(mem_dc, self.palette.muted_text.to_colorref());
                        let mut drc = RECT {
                            left: title_left,
                            top: 0,
                            right: title_right,
                            bottom: tb_h,
                        };
                        let mut wide: Vec<u16> = doc_title.encode_utf16().collect();
                        let _ = DrawTextW(
                            mem_dc,
                            &mut wide,
                            &mut drc,
                            DT_SINGLELINE | DT_CENTER | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                        );
                        let _ = SelectObject(mem_dc, old_f);
                    }

                    // 7. 右上角三个标准系统窗口控制按钮（最小化 / 最大化或还原 / 关闭）
                    let close_red = RgbColor::new(0xE8, 0x11, 0x23);
                    let close_red_brush = CreateSolidBrush(close_red.to_colorref());
                    let is_maximized = IsZoomed(self.hwnd).as_bool();

                    for sys_btn in [TitleBtn::WinMin, TitleBtn::WinMax, TitleBtn::WinClose] {
                        let src = self.titlebar_button_rect(sys_btn, cw);
                        if src.right <= src.left {
                            continue;
                        }
                        let is_hov = self.hovered_btn == Some(sys_btn);
                        let is_prs = self.pressed_btn == Some(sys_btn);
                        let is_close = sys_btn == TitleBtn::WinClose;

                        if is_hov || is_prs {
                            let bg_br = if is_close {
                                close_red_brush
                            } else if is_prs {
                                press_brush
                            } else {
                                hover_brush
                            };
                            let fill_rc = RECT {
                                left: src.left,
                                top: src.top,
                                right: src.right,
                                bottom: (src.bottom - 1).max(src.top),
                            };
                            FillRect(mem_dc, &fill_rc, bg_br);
                        }

                        let icon_col = if is_close && (is_hov || is_prs) {
                            RgbColor::new(255, 255, 255)
                        } else {
                            self.palette.text
                        };
                        let pen_w = self.dip(1).max(1);
                        let icon_pen = CreatePen(PS_SOLID, pen_w, icon_col.to_colorref());
                        let old_p = SelectObject(mem_dc, HGDIOBJ(icon_pen.0));
                        let old_b = SelectObject(mem_dc, hollow_brush);

                        let cx = (src.left + src.right) / 2;
                        let cy = (src.top + src.bottom) / 2;
                        let half = self.dip(5);

                        match sys_btn {
                            TitleBtn::WinMin => {
                                let _ = MoveToEx(mem_dc, cx - half, cy, None);
                                let _ = LineTo(mem_dc, cx + half, cy);
                            }
                            TitleBtn::WinMax => {
                                if is_maximized {
                                    let off = self.dip(2).max(2);
                                    // 后方框顶边与右边
                                    let _ = MoveToEx(
                                        mem_dc,
                                        cx - half + off,
                                        cy - half,
                                        None,
                                    );
                                    let _ = LineTo(mem_dc, cx + half, cy - half);
                                    let _ = LineTo(mem_dc, cx + half, cy + half - off);
                                    // 前方完整小方框
                                    let _ = Rectangle(
                                        mem_dc,
                                        cx - half,
                                        cy - half + off,
                                        cx + half - off + 1,
                                        cy + half + 1,
                                    );
                                } else {
                                    let _ = Rectangle(
                                        mem_dc,
                                        cx - half,
                                        cy - half,
                                        cx + half,
                                        cy + half,
                                    );
                                }
                            }
                            TitleBtn::WinClose => {
                                let _ = MoveToEx(mem_dc, cx - half, cy - half, None);
                                let _ = LineTo(mem_dc, cx + half + 1, cy + half + 1);
                                let _ = MoveToEx(mem_dc, cx + half, cy - half, None);
                                let _ = LineTo(mem_dc, cx - half - 1, cy + half + 1);
                            }
                            _ => {}
                        }

                        let _ = SelectObject(mem_dc, old_b);
                        let _ = SelectObject(mem_dc, old_p);
                        let _ = DeleteObject(HGDIOBJ(icon_pen.0));
                    }

                    let _ = DeleteObject(HGDIOBJ(close_red_brush.0));
                    let _ = DeleteObject(HGDIOBJ(hover_brush.0));
                    let _ = DeleteObject(HGDIOBJ(press_brush.0));

                    let _ = BitBlt(
                        target_dc,
                        0,
                        0,
                        cw,
                        chrome_bot.max(1),
                        Some(mem_dc),
                        0,
                        0,
                        SRCCOPY,
                    );
                    let _ = SelectObject(mem_dc, old_bmp);
                    let _ = DeleteObject(HGDIOBJ(bmp.0));
                }
                let _ = DeleteDC(mem_dc);
            }

            if ch > chrome_bot {
                let body_rect = RECT {
                    left: 0,
                    top: chrome_bot,
                    right: cw,
                    bottom: ch,
                };
                FillRect(target_dc, &body_rect, self.brush_bg);
            }
        }
    }

    /// 绘制与用户参考图一致的「侧边栏折叠/展开」矢量图标（圆角矩形外框 + 左侧竖线 + `<`/`>` 尖括号）。
    unsafe fn draw_sidebar_toggle_icon(&self, hdc: HDC, btn_rc: &RECT, is_open: bool) {
        let stroke_w = ((1.5 * self.dpi_scale()).round() as i32).max(1);
        let color = if is_open {
            self.palette.heading_text
        } else {
            self.palette.text
        };
        unsafe {
            let pen = CreatePen(PS_SOLID, stroke_w, color.to_colorref());
            let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
            let old_brush = SelectObject(hdc, GetStockObject(HOLLOW_BRUSH));

            let cx = (btn_rc.left + btn_rc.right) / 2;
            let cy = (btn_rc.top + btn_rc.bottom) / 2;
            let hw = self.dip(8); // 总宽 16 DIP
            let hh = self.dip(7); // 总高 14 DIP
            let r = self.dip(4);

            let left = cx - hw;
            let top = cy - hh;
            let right = cx + hw;
            let bottom = cy + hh;

            // 1. 圆角外框
            let _ = RoundRect(hdc, left, top, right, bottom, r, r);

            // 2. 左侧边栏竖向分割线
            let split_x = left + self.dip(5);
            let _ = MoveToEx(hdc, split_x, top + 1, None);
            let _ = LineTo(hdc, split_x, bottom - 1);

            // 3. 右侧面板中央尖括号（展开时 `<` 提示收起，折叠时 `>` 提示展开）
            let arrow_cx = (split_x + right) / 2;
            let aw = self.dip(2).max(2);
            let ah = self.dip(3).max(2);
            if is_open {
                let _ = MoveToEx(hdc, arrow_cx + aw / 2, cy - ah, None);
                let _ = LineTo(hdc, arrow_cx - aw, cy);
                let _ = LineTo(hdc, arrow_cx + aw / 2 + 1, cy + ah + 1);
            } else {
                let _ = MoveToEx(hdc, arrow_cx - aw / 2, cy - ah, None);
                let _ = LineTo(hdc, arrow_cx + aw, cy);
                let _ = LineTo(hdc, arrow_cx - aw / 2 - 1, cy + ah + 1);
            }

            let _ = SelectObject(hdc, old_brush);
            let _ = SelectObject(hdc, old_pen);
            let _ = DeleteObject(HGDIOBJ(pen.0));
        }
    }

    /// 统一拦截全局与模式快捷键。
    fn handle_pre_dispatch_keydown(&mut self, msg: &MSG) -> bool {
        let vk = (msg.wParam.0 & 0xFFFF) as u16;
        let ctrl = (unsafe { GetKeyState(i32::from(VK_CONTROL.0)) } as u16 & 0x8000) != 0;
        let shift = (unsafe { GetKeyState(i32::from(VK_SHIFT.0)) } as u16 & 0x8000) != 0;
        let focus = unsafe { GetFocus() };
        let in_find_input =
            self.find_bar_visible && (focus == self.txt_find || msg.hwnd == self.txt_find);
        let in_editor = self.mode == AppMode::Editing
            && self
                .editor
                .as_ref()
                .is_some_and(|e| focus == e.hwnd() || msg.hwnd == e.hwnd());

        // 1. Ctrl+Shift+R 或 F5：热重载 JSON 配置
        if (ctrl && shift && vk == VK_R.0) || (!ctrl && !shift && vk == VK_F5.0) {
            self.reload_config(true);
            return true;
        }

        // 2. Ctrl+E 或 F2：阅读 ↔ 修改模式一键切换（同步滚动位置）
        if (ctrl && !shift && vk == VK_E.0) || (!ctrl && !shift && vk == VK_F2.0) {
            if self.mode == AppMode::Reading {
                self.enter_editing_mode();
            } else {
                self.handle_exit_edit_with_confirm(false);
            }
            return true;
        }

        // 3. Ctrl+S：修改态原子保存并自动切回阅读模式
        if ctrl && !shift && vk == VK_S.0 {
            if self.mode == AppMode::Editing {
                let _ = self.save_and_switch_to_reading();
            }
            return true;
        }

        // 4. Ctrl+O：系统打开文件对话框
        if ctrl && !shift && vk == VK_O.0 {
            self.handle_open_file_dialog();
            return true;
        }

        // 5. Ctrl+F：阅读态唤起页内查找栏
        if ctrl && !shift && vk == VK_F.0 {
            if self.mode == AppMode::Reading {
                self.open_find_bar();
            }
            return true;
        }

        // 6. Ctrl+C：阅读态复制划词选区到剪贴板
        if ctrl && !shift && vk == VK_C.0 && self.mode == AppMode::Reading && !in_find_input {
            if let Some(reader) = &mut self.reader {
                let _ = reader.copy_selection_to_clipboard();
            }
            return true;
        }

        // 7. Esc：优先关闭查找栏；编辑态确认退回阅读模式；阅读态确认退出软件
        if !ctrl && !shift && vk == VK_ESCAPE.0 {
            if self.find_bar_visible {
                self.close_find_bar();
                return true;
            }
            if self.mode == AppMode::Editing {
                self.handle_exit_edit_with_confirm(true);
                return true;
            }
            self.handle_escape_close_app_confirm();
            return true;
        }

        // 8. 查找输入框 Enter / Shift+Enter 跳转匹配
        if in_find_input && !ctrl && vk == VK_RETURN.0 {
            self.step_find(!shift);
            return true;
        }

        // 9. 编辑区 Tab 插入配置的空格缩进（保留撤销栈）
        if in_editor && !ctrl && !shift && vk == VK_TAB.0 {
            if let Some(ed) = &self.editor {
                ed.insert_tab_spaces();
            }
            return true;
        }

        // 10. 阅读态键盘滚动翻页
        if self.mode == AppMode::Reading && !in_find_input && !ctrl {
            let mut scrolled = false;
            if let Some(reader) = &mut self.reader {
                if vk == VK_NEXT.0 {
                    reader.scroll_page(0.85);
                    scrolled = true;
                } else if vk == VK_PRIOR.0 {
                    reader.scroll_page(-0.85);
                    scrolled = true;
                } else if vk == VK_HOME.0 {
                    reader.scroll_to_top();
                    scrolled = true;
                } else if vk == VK_END.0 {
                    reader.scroll_to_bottom();
                    scrolled = true;
                } else if vk == VK_DOWN.0 {
                    reader.scroll_by(64.0);
                    scrolled = true;
                } else if vk == VK_UP.0 {
                    reader.scroll_by(-64.0);
                    scrolled = true;
                }
            }
            if scrolled {
                self.sync_sidebar_active_heading();
                return true;
            }
        }

        false
    }
}

/// 启动 MDReader 主窗口并运行标准 Win32 消息循环直至窗口关闭。
pub fn run_app_window(
    initial_file: Option<PathBuf>,
    cli_config: Option<PathBuf>,
) -> windows::core::Result<()> {
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None)?.into() };
    register_main_window_class(instance)?;

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let config_target_path = resolve_config_path(cli_config.as_deref(), &exe_dir);
    let config_load_result =
        load_config_with_source(Some(&config_target_path), cli_config.is_some());
    let config_watcher = ConfigWatcher::new(Some(config_target_path.clone()));
    let config = config_load_result.config.clone();
    let palette = ThemePalette::from_config(&config);

    let history_path = resolve_history_path(&exe_dir);
    let file_history = FileHistory::load(&history_path, current_epoch_secs());

    let win_w = config.window_width as i32;
    let win_h = config.window_height as i32;
    let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    let pos_x = ((screen_w - win_w) / 2).max(20);
    let pos_y = ((screen_h - win_h) / 2).max(20);

    let state = Box::new(MainWindowState {
        hwnd: HWND::default(),
        instance,
        cli_config_override: cli_config,
        config_target_path,
        config_load_result,
        config_watcher,
        config,
        palette,
        history_path,
        file_history,
        mode: AppMode::Reading,
        current_doc: None,
        doc_stamp: None,
        dirty: false,
        suppress_change_event: false,
        find_bar_visible: false,
        hovered_btn: None,
        pressed_btn: None,
        tracking_mouse_leave: false,
        txt_find: HWND::default(),
        lbl_find_status: HWND::default(),
        ui_font: HFONT::default(),
        ui_font_bold: HFONT::default(),
        cached_font_dpi: 0,
        icon_big: create_app_icon(32),
        icon_small: create_app_icon(16),
        icon_logo: HICON::default(),
        cached_logo_px: 0,
        brush_toolbar: HBRUSH::default(),
        brush_border: HBRUSH::default(),
        brush_bg: HBRUSH::default(),
        brush_input_bg: HBRUSH::default(),
        sidebar: None,
        reader: None,
        editor: None,
    });

    let state_ptr = Box::into_raw(state);
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_ACCEPTFILES,
            MAIN_CLASS_NAME,
            w!("未命名 - 阅读模式 - MDReader"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            pos_x,
            pos_y,
            win_w,
            win_h,
            None,
            None,
            Some(instance),
            Some(state_ptr.cast::<c_void>()),
        )?
    };

    unsafe {
        let state_ref = &mut *state_ptr;
        state_ref.hwnd = hwnd;

        // 通知系统重新应用自定义 WM_NCCALCSIZE 非客户区裁剪
        apply_dwm_frame_and_dark_mode(hwnd, state_ref.palette.is_dark);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
        );

        if !state_ref.icon_big.is_invalid() && !state_ref.icon_big.0.is_null() {
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_BIG as usize)),
                Some(LPARAM(state_ref.icon_big.0 as isize)),
            );
        }
        if !state_ref.icon_small.is_invalid() && !state_ref.icon_small.0.is_null() {
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_SMALL as usize)),
                Some(LPARAM(state_ref.icon_small.0 as isize)),
            );
        }
        if let Err(e) = state_ref.init_children() {
            let _ = DestroyWindow(hwnd);
            return Err(e);
        }
        if let Some(path) = initial_file {
            state_ref.open_file(&path);
        } else {
            state_ref.update_window_title();
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        if let Some(reader) = &state_ref.reader {
            let _ = SetFocus(Some(reader.hwnd()));
        }
    }

    let mut msg = MSG::default();
    loop {
        let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if ret.0 <= 0 {
            break;
        }
        if msg.message == WM_KEYDOWN {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut MainWindowState };
            if !ptr.is_null() {
                let handled = unsafe { (*ptr).handle_pre_dispatch_keydown(&msg) };
                if handled {
                    continue;
                }
            }
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    Ok(())
}

fn register_main_window_class(instance: HINSTANCE) -> windows::core::Result<()> {
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(main_wnd_proc),
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        lpszClassName: MAIN_CLASS_NAME,
        ..Default::default()
    };
    let atom = unsafe { RegisterClassW(&wc) };
    if atom == 0 {
        let err = windows::core::Error::from_thread();
        if err.code() != HRESULT::from_win32(1410)
            && err.code() != HRESULT::from_win32(ERROR_SUCCESS.0)
        {
            return Err(err);
        }
    }
    Ok(())
}

fn show_message_box(
    parent: HWND,
    title: &str,
    message: &str,
    flags: windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE,
) -> windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_RESULT {
    let title_w: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let msg_w: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        MessageBoxW(
            Some(parent),
            PCWSTR(msg_w.as_ptr()),
            PCWSTR(title_w.as_ptr()),
            flags,
        )
    }
}

fn show_open_file_dialog(parent: HWND) -> Option<PathBuf> {
    type GetOpenFileNameWFn = unsafe extern "system" fn(*mut OPENFILENAMEW) -> i32;
    let filter: Vec<u16> =
        "Markdown Files (*.md;*.markdown)\0*.md;*.markdown\0All Files (*.*)\0*.*\0\0"
            .encode_utf16()
            .collect();
    let mut file_buf = vec![0u16; 4096];
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: parent,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        nFilterIndex: 1,
        lpstrFile: PWSTR(file_buf.as_mut_ptr()),
        nMaxFile: file_buf.len() as u32,
        Flags: OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY,
        ..Default::default()
    };
    let ok = unsafe {
        LoadLibraryW(w!("comdlg32.dll"))
            .ok()
            .and_then(|h| GetProcAddress(h, windows::core::s!("GetOpenFileNameW")))
            .is_some_and(|proc| {
                let func: GetOpenFileNameWFn = std::mem::transmute(proc);
                func(&mut ofn) != 0
            })
    };
    if ok {
        let end = file_buf
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(file_buf.len());
        Some(PathBuf::from(std::ffi::OsString::from_wide(&file_buf[..end])))
    } else {
        None
    }
}

fn show_save_file_dialog(parent: HWND) -> Option<PathBuf> {
    type GetSaveFileNameWFn = unsafe extern "system" fn(*mut OPENFILENAMEW) -> i32;
    let filter: Vec<u16> = "Markdown Files (*.md)\0*.md\0All Files (*.*)\0*.*\0\0"
        .encode_utf16()
        .collect();
    let def_ext: Vec<u16> = "md\0".encode_utf16().collect();
    let mut file_buf = vec![0u16; 4096];
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: parent,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        nFilterIndex: 1,
        lpstrFile: PWSTR(file_buf.as_mut_ptr()),
        nMaxFile: file_buf.len() as u32,
        lpstrDefExt: PCWSTR(def_ext.as_ptr()),
        Flags: OFN_EXPLORER | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST,
        ..Default::default()
    };
    let ok = unsafe {
        LoadLibraryW(w!("comdlg32.dll"))
            .ok()
            .and_then(|h| GetProcAddress(h, windows::core::s!("GetSaveFileNameW")))
            .is_some_and(|proc| {
                let func: GetSaveFileNameWFn = std::mem::transmute(proc);
                func(&mut ofn) != 0
            })
    };
    if ok {
        let end = file_buf
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(file_buf.len());
        Some(PathBuf::from(std::ffi::OsString::from_wide(&file_buf[..end])))
    } else {
        None
    }
}

fn extract_first_dropped_file(hdrop: HDROP) -> Option<PathBuf> {
    type DragQueryFileWFn = unsafe extern "system" fn(HDROP, u32, *mut u16, u32) -> u32;
    type DragFinishFn = unsafe extern "system" fn(HDROP);
    unsafe {
        let hmod = LoadLibraryW(w!("shell32.dll")).ok()?;
        let q_proc = GetProcAddress(hmod, windows::core::s!("DragQueryFileW"))?;
        let f_proc = GetProcAddress(hmod, windows::core::s!("DragFinish"))?;
        let drag_query: DragQueryFileWFn = std::mem::transmute(q_proc);
        let drag_finish: DragFinishFn = std::mem::transmute(f_proc);

        let count = drag_query(hdrop, 0xFFFF_FFFF, std::ptr::null_mut(), 0);
        let mut dropped_path = None;
        if count > 0 {
            let len = drag_query(hdrop, 0, std::ptr::null_mut(), 0);
            if len > 0 {
                let mut buf = vec![0u16; len as usize + 2];
                let copied = drag_query(hdrop, 0, buf.as_mut_ptr(), buf.len() as u32);
                if copied > 0 {
                    dropped_path = Some(PathBuf::from(std::ffi::OsString::from_wide(
                        &buf[..copied as usize],
                    )));
                }
            }
        }
        drag_finish(hdrop);
        dropped_path
    }
}

fn lparam_xy(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    (x, y)
}

unsafe extern "system" fn main_wnd_proc(
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

    // 自定义非客户区尺寸：保留左/右/下原生缩放边框与阴影，移除系统顶部标准标题栏
    if msg == WM_NCCALCSIZE && wparam.0 != 0 {
        let params = unsafe { &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS) };
        let orig_top = params.rgrc[0].top;
        let def_res = unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        if unsafe { IsZoomed(hwnd).as_bool() } {
            let mon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if unsafe { GetMonitorInfoW(mon, &mut mi).as_bool() } {
                params.rgrc[0].top = mi.rcWork.top;
            }
        } else {
            params.rgrc[0].top = orig_top;
        }
        return def_res;
    }

    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut MainWindowState };
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    let state = unsafe { &mut *ptr };

    match msg {
        WM_NCHITTEST => {
            let def_hit = unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            let dh = def_hit.0 as u32;
            if matches!(
                dh,
                HTLEFT | HTRIGHT | HTBOTTOM | HTBOTTOMLEFT | HTBOTTOMRIGHT
            ) {
                return def_hit;
            }

            let (sx, sy) = lparam_xy(lparam);
            let mut pt = POINT { x: sx, y: sy };
            let _ = unsafe { ScreenToClient(hwnd, &mut pt) };

            let mut rc = RECT::default();
            let _ = unsafe { GetClientRect(hwnd, &mut rc) };
            let cw = (rc.right - rc.left).max(1);

            // 非最大化时顶部 5 DIP 留作窗口上边缘拖拽缩放热区
            if !unsafe { IsZoomed(hwnd).as_bool() } && pt.y >= 0 && pt.y < state.dip(5) {
                if pt.x < state.dip(8) {
                    return LRESULT(HTTOPLEFT as isize);
                }
                if pt.x >= cw - state.dip(8) {
                    return LRESULT(HTTOPRIGHT as isize);
                }
                return LRESULT(HTTOP as isize);
            }

            if pt.y >= 0 && pt.y < state.titlebar_height_px() {
                if state.hit_test_titlebar_btn(pt.x, pt.y, cw).is_some() {
                    return LRESULT(HTCLIENT as isize);
                }
                return LRESULT(HTCAPTION as isize);
            }

            LRESULT(HTCLIENT as isize)
        }
        WM_NCMOUSEMOVE => {
            if state.hovered_btn.is_some() {
                state.hovered_btn = None;
                state.invalidate_titlebar();
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_SETCURSOR => {
            let hit = (lparam.0 & 0xFFFF) as u32;
            if hit == HTCLIENT {
                let cursor_id = if state.hovered_btn.is_some() {
                    IDC_HAND
                } else {
                    IDC_ARROW
                };
                if let Ok(cur) = unsafe { LoadCursorW(None, cursor_id) } {
                    unsafe {
                        SetCursor(Some(cur));
                    }
                    return LRESULT(1);
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_ERASEBKGND => {
            state.paint_chrome(HDC(wparam.0 as *mut c_void));
            LRESULT(1)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            state.paint_chrome(hdc);
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_PRINTCLIENT => {
            state.paint_chrome(HDC(wparam.0 as *mut c_void));
            LRESULT(0)
        }
        WM_SIZE => {
            state.layout_children();
            state.invalidate_titlebar();
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let suggested = unsafe { &*(lparam.0 as *const RECT) };
            unsafe {
                let _ = MoveWindow(
                    hwnd,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    true,
                );
            }
            state.ensure_dpi_resources();
            state.layout_children();
            state.invalidate_titlebar();
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => {
            let hdc = HDC(wparam.0 as *mut c_void);
            unsafe {
                SetTextColor(hdc, state.palette.text.to_colorref());
                SetBkColor(hdc, state.palette.toolbar_bg.to_colorref());
                SetBkMode(hdc, TRANSPARENT);
            }
            LRESULT(state.brush_toolbar.0 as isize)
        }
        WM_CTLCOLOREDIT => {
            let hdc = HDC(wparam.0 as *mut c_void);
            let input_bg: COLORREF = if state.palette.is_dark {
                RgbColor::new(0x18, 0x18, 0x1B).to_colorref()
            } else {
                RgbColor::new(0xFF, 0xFF, 0xFF).to_colorref()
            };
            unsafe {
                SetTextColor(hdc, state.palette.text.to_colorref());
                SetBkColor(hdc, input_bg);
            }
            LRESULT(state.brush_input_bg.0 as isize)
        }
        WM_COMMAND => {
            let ctrl_id = (wparam.0 & 0xFFFF) as u16;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            match (ctrl_id, code) {
                (crate::ui::IDC_EDITOR, EN_CHANGE) => {
                    if !state.suppress_change_event && state.mode == AppMode::Editing && !state.dirty
                    {
                        state.dirty = true;
                        state.update_window_title();
                    }
                    LRESULT(0)
                }
                (IDC_FIND_INPUT, EN_CHANGE) => {
                    if state.find_bar_visible {
                        state.on_find_text_changed();
                    }
                    LRESULT(0)
                }
                _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
            }
        }
        WM_APP_SIDEBAR_OUTLINE_JUMP => {
            let block_idx = wparam.0;
            let start_line = lparam.0.max(0) as usize;
            if state.mode == AppMode::Reading {
                if let Some(reader) = &mut state.reader {
                    reader.scroll_to_block(block_idx);
                    let _ = unsafe { SetFocus(Some(reader.hwnd())) };
                }
                state.sync_sidebar_active_heading();
            } else if let Some(editor) = &state.editor {
                editor.scroll_to_source_line(start_line);
                let _ = unsafe { SetFocus(Some(editor.hwnd())) };
            }
            LRESULT(0)
        }
        WM_APP_SIDEBAR_HISTORY_OPEN => {
            let idx = wparam.0;
            if let Some(entry) = state.file_history.entries.get(idx).cloned() {
                let target = PathBuf::from(&entry.path);
                let already_same = state
                    .current_doc
                    .as_ref()
                    .is_some_and(|d| d.path == target && state.mode == AppMode::Reading);
                if !already_same && state.confirm_discard_or_save_if_dirty() {
                    state.open_file(&target);
                }
            }
            LRESULT(0)
        }
        WM_APP_READER_SCROLLED => {
            state.sync_sidebar_active_heading();
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            if state.mode == AppMode::Reading
                && let Some(reader) = &mut state.reader
            {
                let delta = ((wparam.0 >> 16) & 0xFFFF) as i16;
                reader.handle_wheel_delta(delta);
                state.sync_sidebar_active_heading();
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_MOUSEMOVE => {
            if !state.tracking_mouse_leave {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                if unsafe { TrackMouseEvent(&mut tme) }.is_ok() {
                    state.tracking_mouse_leave = true;
                }
            }

            let (x, y) = lparam_xy(lparam);
            let mut rc = RECT::default();
            let _ = unsafe { GetClientRect(hwnd, &mut rc) };
            let cw = (rc.right - rc.left).max(1);

            let next_hov = state.hit_test_titlebar_btn(x, y, cw);
            if next_hov != state.hovered_btn {
                state.hovered_btn = next_hov;
                state.invalidate_titlebar();
            }

            let top_offset = state.titlebar_height_px() + state.findbar_height_px();
            let sb_w = state.sidebar_width_px();
            if state.pressed_btn.is_none()
                && state.mode == AppMode::Reading
                && y >= top_offset
                && let Some(reader) = &mut state.reader
            {
                let rx = (x - sb_w).max(0);
                let ry = (y - top_offset).max(0);
                reader.forward_mouse_move(rx, ry);
                return LRESULT(0);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            state.tracking_mouse_leave = false;
            if state.hovered_btn.is_some() {
                state.hovered_btn = None;
                state.invalidate_titlebar();
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = lparam_xy(lparam);
            let mut rc = RECT::default();
            let _ = unsafe { GetClientRect(hwnd, &mut rc) };
            let cw = (rc.right - rc.left).max(1);

            if let Some(btn) = state.hit_test_titlebar_btn(x, y, cw) {
                state.pressed_btn = Some(btn);
                let _ = unsafe { SetCapture(hwnd) };
                state.invalidate_titlebar();
                return LRESULT(0);
            }

            let top_offset = state.titlebar_height_px() + state.findbar_height_px();
            let sb_w = state.sidebar_width_px();
            if state.mode == AppMode::Reading
                && y >= top_offset
                && x >= sb_w
                && let Some(reader) = &mut state.reader
            {
                reader.forward_mouse_down(x - sb_w, y - top_offset);
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_LBUTTONUP => {
            let (x, y) = lparam_xy(lparam);
            if let Some(pressed) = state.pressed_btn.take() {
                let _ = unsafe { ReleaseCapture() };
                let mut rc = RECT::default();
                let _ = unsafe { GetClientRect(hwnd, &mut rc) };
                let cw = (rc.right - rc.left).max(1);
                let released_on = state.hit_test_titlebar_btn(x, y, cw);
                state.hovered_btn = released_on;
                state.invalidate_titlebar();
                if released_on == Some(pressed) {
                    state.activate_titlebar_button(pressed);
                }
                return LRESULT(0);
            }

            let top_offset = state.titlebar_height_px() + state.findbar_height_px();
            let sb_w = state.sidebar_width_px();
            if state.mode == AppMode::Reading
                && let Some(reader) = &mut state.reader
            {
                reader.forward_mouse_up((x - sb_w).max(0), (y - top_offset).max(0));
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_DROPFILES => {
            let hdrop = HDROP(wparam.0 as *mut c_void);
            if let Some(path) = extract_first_dropped_file(hdrop)
                && state.confirm_discard_or_save_if_dirty()
            {
                state.open_file(&path);
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            let active_state = (wparam.0 & 0xFFFF) as u16;
            if active_state != 0 {
                state.poll_external_changes();
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_TIMER => {
            if wparam.0 == TIMER_WATCH_CHANGES {
                state.poll_external_changes();
            }
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            state.on_system_theme_changed();
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_CLOSE => {
            if !state.confirm_discard_or_save_if_dirty() {
                return LRESULT(0);
            }
            state.persist_window_geometry_if_safe();
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_WATCH_CHANGES);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let boxed = Box::from_raw(ptr);
                for b in [
                    boxed.brush_toolbar,
                    boxed.brush_border,
                    boxed.brush_bg,
                    boxed.brush_input_bg,
                ] {
                    if !b.is_invalid() && !b.0.is_null() {
                        let _ = DeleteObject(HGDIOBJ(b.0));
                    }
                }
                for f in [boxed.ui_font, boxed.ui_font_bold] {
                    if !f.is_invalid() && !f.0.is_null() {
                        let _ = DeleteObject(HGDIOBJ(f.0));
                    }
                }
                for ic in [boxed.icon_big, boxed.icon_small, boxed.icon_logo] {
                    if !ic.is_invalid() && !ic.0.is_null() {
                        let _ = DestroyIcon(ic);
                    }
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::{APP_ICON_ICO, select_ico_png_frame};

    fn png_dimensions(png: &[u8]) -> Option<(u32, u32)> {
        if png.len() < 24 || !png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return None;
        }
        let w = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
        let h = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
        Some((w, h))
    }

    #[test]
    fn test_select_ico_png_frame_picks_best_resolution() {
        assert_eq!(
            select_ico_png_frame(APP_ICON_ICO, 16).and_then(png_dimensions),
            Some((16, 16))
        );
        assert_eq!(
            select_ico_png_frame(APP_ICON_ICO, 20).and_then(png_dimensions),
            Some((20, 20))
        );
        assert_eq!(
            select_ico_png_frame(APP_ICON_ICO, 25).and_then(png_dimensions),
            Some((32, 32))
        );
        assert_eq!(
            select_ico_png_frame(APP_ICON_ICO, 32).and_then(png_dimensions),
            Some((32, 32))
        );
        assert_eq!(
            select_ico_png_frame(APP_ICON_ICO, 48).and_then(png_dimensions),
            Some((48, 48))
        );
        assert_eq!(
            select_ico_png_frame(APP_ICON_ICO, 256).and_then(png_dimensions),
            Some((256, 256))
        );
        assert_eq!(
            select_ico_png_frame(APP_ICON_ICO, 512).and_then(png_dimensions),
            Some((256, 256))
        );
    }

    #[test]
    fn test_select_ico_png_frame_handles_corrupt_or_truncated_inputs() {
        assert!(select_ico_png_frame(&[], 32).is_none());
        assert!(select_ico_png_frame(&[0, 0, 1, 0, 1, 0], 32).is_none());
        assert!(select_ico_png_frame(&APP_ICON_ICO[..12], 32).is_none());
    }
}
