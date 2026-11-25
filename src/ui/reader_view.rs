//! Direct2D / DirectWrite / WIC 纯原生 Markdown 阅读渲染引擎（归属 Milestone 4）。
//!
//! 核心设计目标（严格遵循 `内存 > UI 美化 > 数据安全`）：
//! 1. 极致低内存：单线程 `ID2D1Factory` + 系统共享 `IDWriteFactory`，仅存储轻量块高度索引，
//!    `IDWriteTextLayout` 使用有界 LRU 缓存（上限 96），绘制时二分裁剪仅渲染当前视口可见块，
//!    WIC 图片解码工厂按需懒加载且位图缩放缓存有界；
//! 2. 精致阅读美学（对标 `mdfiler.com`）：居中限宽版心、ClearType 亚像素抗锯齿、精确行高、
//!    完整 GFM 渲染（H1–H6 阶梯与底线、行内代码胶囊、多级/任务列表矢量复选框、引用块、
//!    GFM 斑马纹表格、6 色代码词法高亮、本地图片与破图占位、水平分割线）；
//! 3. 完整阅读交互：滚轮与键盘翻页、跨块鼠标划词高亮与 `Ctrl+C` 复制、安全超链接点击、
//!    `Ctrl+F` 页内查找高亮与跳转；
//! 4. 后台离屏截图契约：处理 `WM_PRINTCLIENT` 经由 `ID2D1DCRenderTarget` 复走同一绘制管线，
//!    保证 `PrintWindow(..., PW_RENDERFULLCONTENT)` 始终清晰成像无白屏。

use std::collections::HashMap;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{
    COLORREF, ERROR_SUCCESS, GlobalFree, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT,
    WPARAM,
};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED,
    D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_PROPERTIES, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
    D2D1CreateFactory, ID2D1Bitmap, ID2D1DCRenderTarget, ID2D1Factory, ID2D1RenderTarget,
    ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_HIT_TEST_METRICS, DWRITE_LINE_SPACING_METHOD_UNIFORM,
    DWRITE_READING_DIRECTION_LEFT_TO_RIGHT, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_RANGE,
    DWRITE_WORD_WRAPPING_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
    IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint, HBITMAP, HDC, HGDIOBJ,
    InvalidateRect, PAINTSTRUCT, SRCCOPY, SelectObject,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Memory::{GHND, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_DBLCLKS, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW,
    GWLP_USERDATA, GetClientRect, GetWindowLongPtrW, HMENU, IDC_ARROW, IDC_HAND, IDC_IBEAM,
    LoadCursorW, RegisterClassW, SW_SHOWNORMAL, SetCursor, SetWindowLongPtrW, WM_CREATE,
    WM_ERASEBKGND, WM_GETOBJECT, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_NCDESTROY, WM_PAINT, WM_PRINT, WM_PRINTCLIENT, WM_SETCURSOR, WM_SIZE, WNDCLASSW, WS_CHILD,
    WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_VISIBLE,
};
use windows::core::{Interface, PCWSTR, w};
use windows_numerics::{Matrix3x2, Vector2};

use crate::core::config::AppConfig;
use crate::core::markdown::{BlockKind, ParsedDocument, RichSpan, TableAlign, TokenKind};
use crate::ui::theme::{RgbColor, ThemePalette, primary_font_family};
use crate::ui::uia;

/// 阅读画布窗口类名称。
const READER_CLASS_NAME: PCWSTR = w!("MDReaderCanvas");

/// LRU `IDWriteTextLayout` 缓存上限（控制 200KB+ 长文档的 COM 内存峰值）。
const MAX_CACHED_LAYOUTS: usize = 96;

/// 图片解码缓存上限（控制大图场景下的常驻内存）。
const MAX_CACHED_IMAGES: usize = 16;

/// 单张图片最大解码宽度（像素），防止 4K+ 超大图占用过多内存。
const MAX_IMAGE_DECODE_DIM: u32 = 1440;

/// 文档逻辑位置（块索引 + 展平文本中的 UTF-16 码元偏移）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct DocPos {
    block: usize,
    utf16_offset: u32,
}

/// 页内查找命中项。
#[derive(Debug, Clone)]
struct FindMatch {
    block: usize,
    /// 对于非表格块，`cell_idx == None`；表格块记录 `Some((row, col))`。
    cell_idx: Option<(usize, usize)>,
    utf16_start: u32,
    utf16_len: u32,
    approx_y: f32,
}

/// 单个富文本片段在展平文本中的 UTF-16 范围与样式属性。
#[derive(Debug, Clone)]
struct SpanRange {
    start: u32,
    len: u32,
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link_url: Option<String>,
    highlight: Option<TokenKind>,
}

/// 展平后的块文本与内联样式范围（仅临时构建或随 LRU 缓存，不长期囤积）。
struct FlattenedSpans {
    text: String,
    utf16_len: u32,
    ranges: Vec<SpanRange>,
}

/// 每个语义块的轻量几何索引（数十字节/块）。
#[derive(Debug, Clone)]
struct BlockMetric {
    y_top: f32,
    height: f32,
    text_x: f32,
    text_y: f32,
    text_w: f32,
    utf16_len: u32,
}

/// 表格列宽与行高预计算缓存。
#[derive(Debug, Clone)]
struct TableMetric {
    col_widths: Vec<f32>,
    row_heights: Vec<f32>,
    table_x: f32,
    table_w: f32,
}

/// 缓存的普通块排版布局。
struct CachedLayoutEntry {
    layout: IDWriteTextLayout,
    ranges: Vec<SpanRange>,
    last_used: u64,
}

/// 解码后预乘 BGRA 图像缓存（RenderTarget 无关，支持屏幕与 `WM_PRINTCLIENT` 共享）。
enum CachedImage {
    Loaded {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
        last_used: u64,
    },
    Failed {
        reason: String,
        last_used: u64,
    },
}

/// 每帧针对指定 `ID2D1RenderTarget` 创建的画刷集合（杜绝跨 RenderTarget 共享 Brush）。
struct FrameBrushes {
    text: ID2D1SolidColorBrush,
    heading: ID2D1SolidColorBrush,
    muted: ID2D1SolidColorBrush,
    code_bg: ID2D1SolidColorBrush,
    code_border: ID2D1SolidColorBrush,
    quote_bg: ID2D1SolidColorBrush,
    quote_bar: ID2D1SolidColorBrush,
    link: ID2D1SolidColorBrush,
    border: ID2D1SolidColorBrush,
    table_header_bg: ID2D1SolidColorBrush,
    table_stripe_bg: ID2D1SolidColorBrush,
    selection: ID2D1SolidColorBrush,
    find_match: ID2D1SolidColorBrush,
    find_active: ID2D1SolidColorBrush,
    error_text: ID2D1SolidColorBrush,
    scrollbar: ID2D1SolidColorBrush,
    check_mark: ID2D1SolidColorBrush,
    tok_plain: ID2D1SolidColorBrush,
    tok_keyword: ID2D1SolidColorBrush,
    tok_string: ID2D1SolidColorBrush,
    tok_comment: ID2D1SolidColorBrush,
    tok_number: ID2D1SolidColorBrush,
    tok_type_or_fn: ID2D1SolidColorBrush,
}

impl FrameBrushes {
    fn new(rt: &ID2D1RenderTarget, p: &ThemePalette) -> windows::core::Result<Self> {
        let mk = |c: RgbColor, a: f32| unsafe { rt.CreateSolidColorBrush(&c.to_d2d(a), None) };
        let sel_alpha = if p.is_dark { 0.52 } else { 0.42 };
        let find_alpha = if p.is_dark { 0.65 } else { 0.58 };
        let active_alpha = if p.is_dark { 0.85 } else { 0.72 };
        Ok(Self {
            text: mk(p.text, 1.0)?,
            heading: mk(p.heading_text, 1.0)?,
            muted: mk(p.muted_text, 1.0)?,
            code_bg: mk(p.code_bg, 1.0)?,
            code_border: mk(p.code_border, 1.0)?,
            quote_bg: mk(p.quote_bg, 1.0)?,
            quote_bar: mk(p.quote_bar, 1.0)?,
            link: mk(p.link, 1.0)?,
            border: mk(p.border, 1.0)?,
            table_header_bg: mk(p.table_header_bg, 1.0)?,
            table_stripe_bg: mk(p.table_stripe_bg, 1.0)?,
            selection: mk(p.selection_bg, sel_alpha)?,
            find_match: mk(p.find_match_bg, find_alpha)?,
            find_active: mk(p.find_active_bg, active_alpha)?,
            error_text: mk(p.error_text, 1.0)?,
            scrollbar: mk(p.muted_text, if p.is_dark { 0.42 } else { 0.34 })?,
            check_mark: mk(RgbColor::new(0xFF, 0xFF, 0xFF), 1.0)?,
            tok_plain: mk(p.tok_plain, 1.0)?,
            tok_keyword: mk(p.tok_keyword, 1.0)?,
            tok_string: mk(p.tok_string, 1.0)?,
            tok_comment: mk(p.tok_comment, 1.0)?,
            tok_number: mk(p.tok_number, 1.0)?,
            tok_type_or_fn: mk(p.tok_type_or_fn, 1.0)?,
        })
    }

    fn token_brush(&self, tok: TokenKind) -> &ID2D1SolidColorBrush {
        match tok {
            TokenKind::Plain => &self.tok_plain,
            TokenKind::Keyword => &self.tok_keyword,
            TokenKind::String => &self.tok_string,
            TokenKind::Comment => &self.tok_comment,
            TokenKind::Number => &self.tok_number,
            TokenKind::TypeOrFn => &self.tok_type_or_fn,
        }
    }
}

/// 双缓冲 32bpp 内存位图（配合 `ID2D1DCRenderTarget` 实现零闪烁、零 GPU 驱动加载且屏幕/截图 100% 同源）。
struct SoftwareBackBuffer {
    mem_dc: HDC,
    bitmap: HBITMAP,
    old_obj: HGDIOBJ,
    width: i32,
    height: i32,
}

impl SoftwareBackBuffer {
    fn new(width: i32, height: i32) -> Option<Self> {
        let w = width.max(1);
        let h = height.max(1);
        let mem_dc = unsafe { CreateCompatibleDC(None) };
        if mem_dc.is_invalid() || mem_dc.0.is_null() {
            return None;
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down 32bpp DIB
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let Ok(bitmap) =
            (unsafe { CreateDIBSection(Some(mem_dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) })
        else {
            let _ = unsafe { DeleteDC(mem_dc) };
            return None;
        };
        let old_obj = unsafe { SelectObject(mem_dc, HGDIOBJ(bitmap.0)) };
        Some(Self {
            mem_dc,
            bitmap,
            old_obj,
            width: w,
            height: h,
        })
    }
}

impl Drop for SoftwareBackBuffer {
    fn drop(&mut self) {
        unsafe {
            if !self.mem_dc.is_invalid() && !self.mem_dc.0.is_null() {
                let _ = SelectObject(self.mem_dc, self.old_obj);
                if !self.bitmap.is_invalid() && !self.bitmap.0.is_null() {
                    let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
                }
                let _ = DeleteDC(self.mem_dc);
            }
        }
    }
}

/// 纯原生 Direct2D 阅读画布核心状态对象（堆分配，指针存入 `GWLP_USERDATA`）。
pub struct ReaderViewState {
    hwnd: HWND,
    d2d_factory: ID2D1Factory,
    dwrite_factory: IDWriteFactory,
    wic_factory: Option<IWICImagingFactory>,
    dc_rt: Option<ID2D1DCRenderTarget>,
    back_buffer: Option<SoftwareBackBuffer>,

    config: AppConfig,
    palette: ThemePalette,
    doc: ParsedDocument,
    doc_dir: Option<PathBuf>,
    is_empty_welcome: bool,
    error_banner: Option<String>,

    dpi_scale: f32,
    client_w: f32,
    client_h: f32,
    scroll_y: f32,
    content_total_h: f32,
    layout_dirty: bool,

    metrics: Vec<BlockMetric>,
    table_metrics: HashMap<usize, TableMetric>,
    layout_cache: HashMap<usize, CachedLayoutEntry>,
    cache_tick: u64,
    image_cache: HashMap<String, CachedImage>,
    image_tick: u64,

    // 鼠标划词、拖拽与超链接悬停
    selecting: bool,
    drag_start_dip: (f32, f32),
    drag_moved: bool,
    sel_anchor: Option<DocPos>,
    sel_focus: Option<DocPos>,
    hovered_link: Option<String>,

    // 页内查找
    find_query: String,
    find_matches: Vec<FindMatch>,
    active_match: Option<usize>,
}

/// 包装阅读画布窗口句柄及所有权控制接口。
pub struct ReaderView {
    hwnd: HWND,
    state_ptr: *mut ReaderViewState,
}

impl ReaderView {
    /// 创建 Direct2D 阅读画布子窗口。
    pub fn create(
        parent: HWND,
        instance: HINSTANCE,
        control_id: u16,
        config: &AppConfig,
        palette: ThemePalette,
    ) -> windows::core::Result<Self> {
        register_reader_window_class(instance)?;

        let d2d_factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let dwrite_factory: IDWriteFactory =
            unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };

        let state = Box::new(ReaderViewState {
            hwnd: HWND::default(),
            d2d_factory,
            dwrite_factory,
            wic_factory: None,
            dc_rt: None,
            back_buffer: None,
            config: config.clone(),
            palette,
            doc: ParsedDocument::default(),
            doc_dir: None,
            is_empty_welcome: true,
            error_banner: None,
            dpi_scale: 1.0,
            client_w: 800.0,
            client_h: 600.0,
            scroll_y: 0.0,
            content_total_h: 0.0,
            layout_dirty: true,
            metrics: Vec::new(),
            table_metrics: HashMap::new(),
            layout_cache: HashMap::new(),
            cache_tick: 0,
            image_cache: HashMap::new(),
            image_tick: 0,
            selecting: false,
            drag_start_dip: (0.0, 0.0),
            drag_moved: false,
            sel_anchor: None,
            sel_focus: None,
            hovered_link: None,
            find_query: String::new(),
            find_matches: Vec::new(),
            active_match: None,
        });

        let state_ptr = Box::into_raw(state);
        let hwnd = unsafe {
            CreateWindowExW(
                Default::default(),
                READER_CLASS_NAME,
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
                0,
                0,
                800,
                600,
                Some(parent),
                Some(HMENU(control_id as usize as *mut c_void)),
                Some(instance),
                Some(state_ptr.cast::<c_void>()),
            )?
        };

        unsafe {
            (*state_ptr).hwnd = hwnd;
            (*state_ptr).refresh_dpi();
        }

        Ok(Self { hwnd, state_ptr })
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    fn state_mut(&mut self) -> &mut ReaderViewState {
        unsafe { &mut *self.state_ptr }
    }

    fn state(&self) -> &ReaderViewState {
        unsafe { &*self.state_ptr }
    }

    /// 设置待渲染的已解析文档与文档目录，并重置滚动/选区状态。
    pub fn set_document(
        &mut self,
        doc: ParsedDocument,
        doc_dir: Option<PathBuf>,
        is_empty_welcome: bool,
        reset_scroll: bool,
    ) {
        let s = self.state_mut();
        s.doc = doc;
        s.doc_dir = doc_dir;
        s.is_empty_welcome = is_empty_welcome;
        s.error_banner = None;
        s.sel_anchor = None;
        s.sel_focus = None;
        s.hovered_link = None;
        s.layout_cache.clear();
        s.table_metrics.clear();
        s.image_cache.clear();
        s.layout_dirty = true;
        if reset_scroll {
            s.scroll_y = 0.0;
        }
        s.rebuild_layout_metrics_if_needed();
        if !s.find_query.is_empty() {
            let q = s.find_query.clone();
            s.update_find_query(&q, false);
        } else {
            s.find_matches.clear();
            s.active_match = None;
        }
        s.clamp_scroll();
        s.invalidate();
    }

    /// 设置错误横幅（如命令行传入不存在路径或文件无法读取时在阅读区优雅展示）。
    pub fn set_error_banner(&mut self, msg: String) {
        let s = self.state_mut();
        s.doc = ParsedDocument::default();
        s.doc_dir = None;
        s.is_empty_welcome = false;
        s.error_banner = Some(msg);
        s.sel_anchor = None;
        s.sel_focus = None;
        s.find_matches.clear();
        s.active_match = None;
        s.scroll_y = 0.0;
        s.layout_cache.clear();
        s.table_metrics.clear();
        s.layout_dirty = true;
        s.invalidate();
    }

    /// 更新配置与主题调色板并触发重排重绘。
    pub fn apply_config(&mut self, config: &AppConfig, palette: ThemePalette) {
        let s = self.state_mut();
        s.config = config.clone();
        s.palette = palette;
        s.layout_cache.clear();
        s.table_metrics.clear();
        s.layout_dirty = true;
        s.rebuild_layout_metrics_if_needed();
        s.clamp_scroll();
        s.invalidate();
    }

    /// 调整滚动偏移（正值向下，负值向上）。
    pub fn scroll_by(&mut self, delta_dip: f32) {
        let s = self.state_mut();
        s.rebuild_layout_metrics_if_needed();
        let old = s.scroll_y;
        s.scroll_y += delta_dip;
        s.clamp_scroll();
        if (s.scroll_y - old).abs() > 0.1 {
            s.invalidate();
        }
    }

    /// 滚轮事件转发（支持主窗口转发 `WM_MOUSEWHEEL`）。
    pub fn handle_wheel_delta(&mut self, wheel_delta: i16) {
        let lines = -(f32::from(wheel_delta) / 120.0);
        let step = (self.state().config.font_size * self.state().config.line_height * 3.0).max(56.0);
        self.scroll_by(lines * step);
    }

    /// 键盘翻页导航（PageUp / PageDown / Home / End / Up / Down）。
    pub fn scroll_page(&mut self, pages: f32) {
        let h = (self.state().client_h - 60.0).max(160.0);
        self.scroll_by(pages * h);
    }

    pub fn scroll_to_top(&mut self) {
        let s = self.state_mut();
        s.scroll_y = 0.0;
        s.invalidate();
    }

    pub fn scroll_to_bottom(&mut self) {
        let s = self.state_mut();
        s.rebuild_layout_metrics_if_needed();
        s.scroll_y = (s.content_total_h - s.client_h).max(0.0);
        s.invalidate();
    }

    /// 接收外部转发的鼠标左键按下/移动/抬起（用于无前景测试中消息直接投递到主窗口场景）。
    pub fn forward_mouse_down(&mut self, x_px: i32, y_px: i32) {
        self.state_mut().on_mouse_down(x_px, y_px);
    }

    pub fn forward_mouse_move(&mut self, x_px: i32, y_px: i32) {
        self.state_mut().on_mouse_move(x_px, y_px);
    }

    pub fn forward_mouse_up(&mut self, x_px: i32, y_px: i32) {
        self.state_mut().on_mouse_up(x_px, y_px);
    }

    /// 复制当前划词选区到系统剪贴板；有有效选区并写入成功返回 `true`。
    pub fn copy_selection_to_clipboard(&mut self) -> bool {
        let text = self.state_mut().extract_selected_text();
        if text.is_empty() {
            return false;
        }
        write_clipboard_unicode(&text)
    }

    /// 更新页内查找关键词并返回最新状态文案（如 `"1/3"`、`"0 命中"`、`""`）。
    pub fn set_find_query(&mut self, query: &str) -> String {
        let s = self.state_mut();
        s.update_find_query(query, true);
        s.find_status_text()
    }

    /// 查找跳转到上一处或下一处并返回最新状态文案。
    pub fn step_find_match(&mut self, forward: bool) -> String {
        let s = self.state_mut();
        s.step_find_match(forward);
        s.find_status_text()
    }

    /// 清除页内查找高亮。
    pub fn clear_find(&mut self) {
        let s = self.state_mut();
        s.find_query.clear();
        s.find_matches.clear();
        s.active_match = None;
        s.invalidate();
    }

    pub fn find_status_text(&self) -> String {
        self.state().find_status_text()
    }

    pub fn has_error_banner(&self) -> bool {
        self.state().error_banner.is_some()
    }

    /// 返回当前滚动偏移（DIP）。
    pub fn scroll_offset(&self) -> f32 {
        self.state().scroll_y
    }

    /// 根据当前视口顶部滚动位置映射回 0-based 源码行号（含块内比例插值）。
    pub fn current_source_line(&mut self) -> usize {
        let s = self.state_mut();
        s.rebuild_layout_metrics_if_needed();
        if s.scroll_y <= 2.0 || s.metrics.is_empty() || s.doc.blocks.is_empty() {
            return 0;
        }
        let max_scroll = (s.content_total_h - s.client_h).max(0.0);
        if max_scroll > 0.0 && s.scroll_y >= max_scroll - 2.0 {
            return s.doc.total_lines.saturating_sub(1);
        }

        let anchor_y = s.scroll_y + 16.0;
        let idx = s
            .metrics
            .partition_point(|m| m.y_top + m.height < anchor_y)
            .min(s.metrics.len() - 1);

        let m = &s.metrics[idx];
        let b = &s.doc.blocks[idx];
        let next_start = s
            .doc
            .blocks
            .get(idx + 1)
            .map(|nb| nb.start_line)
            .unwrap_or_else(|| b.end_line.saturating_add(1));
        let span_lines = next_start.saturating_sub(b.start_line);
        let frac = ((anchor_y - m.y_top) / m.height.max(1.0)).clamp(0.0, 1.0);
        b.start_line + ((span_lines as f32) * frac).round() as usize
    }

    /// 根据 0-based 源码行号反向定位阅读画布的滚动位置（含块内比例插值）。
    pub fn scroll_to_source_line(&mut self, line: usize) {
        let s = self.state_mut();
        s.rebuild_layout_metrics_if_needed();
        if line == 0 || s.metrics.is_empty() || s.doc.blocks.is_empty() {
            s.scroll_y = 0.0;
            s.invalidate();
            return;
        }
        if line >= s.doc.total_lines.saturating_sub(1) {
            s.scroll_y = (s.content_total_h - s.client_h).max(0.0);
            s.invalidate();
            return;
        }

        let pos = s.doc.blocks.partition_point(|b| b.start_line <= line);
        let idx = pos.saturating_sub(1).min(s.metrics.len() - 1);
        let m = &s.metrics[idx];
        let b = &s.doc.blocks[idx];
        let next_start = s
            .doc
            .blocks
            .get(idx + 1)
            .map(|nb| nb.start_line)
            .unwrap_or_else(|| b.end_line.saturating_add(1));
        let span_lines = next_start.saturating_sub(b.start_line).max(1);
        let frac = ((line.saturating_sub(b.start_line) as f32) / (span_lines as f32)).clamp(0.0, 1.0);

        s.scroll_y = (m.y_top + m.height * frac - 16.0).max(0.0);
        s.clamp_scroll();
        s.invalidate();
    }

    /// 精确滚动到指定块索引顶部（供左侧大纲目录点击跳转使用）。
    pub fn scroll_to_block(&mut self, block_index: usize) {
        let s = self.state_mut();
        s.rebuild_layout_metrics_if_needed();
        if let Some(m) = s.metrics.get(block_index) {
            s.scroll_y = (m.y_top - 16.0).max(0.0);
            s.clamp_scroll();
            s.invalidate();
        }
    }

    /// 返回当前视口所属章节标题的 `block_index`（供大纲实时高亮当前标题）。
    pub fn active_heading_block(&mut self) -> Option<usize> {
        let s = self.state_mut();
        s.rebuild_layout_metrics_if_needed();
        let anchor_y = s.scroll_y + 48.0;
        let mut first_heading = None;
        let mut active = None;
        for (idx, (b, m)) in s.doc.blocks.iter().zip(s.metrics.iter()).enumerate() {
            if matches!(b.kind, BlockKind::Heading { .. }) {
                if first_heading.is_none() {
                    first_heading = Some(idx);
                }
                if m.y_top <= anchor_y {
                    active = Some(idx);
                } else {
                    break;
                }
            }
        }
        active.or(first_heading)
    }

    /// 强制刷新重绘。
    pub fn invalidate(&self) {
        self.state().invalidate();
    }
}

impl ReaderViewState {
    fn refresh_dpi(&mut self) {
        let dpi = unsafe { GetDpiForWindow(self.hwnd) };
        self.dpi_scale = if dpi > 0 { dpi as f32 / 96.0 } else { 1.0 };
    }

    fn px_to_dip(&self, px: i32) -> f32 {
        px as f32 / self.dpi_scale.max(0.5)
    }

    fn invalidate(&self) {
        if !self.hwnd.is_invalid() && !self.hwnd.0.is_null() {
            let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
        }
    }

    /// 计算居中限宽阅读版心的 `(left, width, padding_v)`。
    fn page_geometry(&self) -> (f32, f32, f32) {
        let pad = self.config.page_padding as f32;
        let max_w = self.config.max_width as f32;
        let avail = (self.client_w - pad * 2.0).max(200.0);
        let content_w = avail.min(max_w).min((self.client_w - 24.0).max(160.0));
        let left = ((self.client_w - content_w) * 0.5).max(12.0);
        let top_pad = (pad * 0.72).clamp(18.0, 52.0);
        (left, content_w, top_pad)
    }

    fn clamp_scroll(&mut self) {
        let max_scroll = (self.content_total_h - self.client_h).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);
    }

    fn body_font_name(&self) -> &str {
        primary_font_family(&self.config.font_family, "Microsoft YaHei UI")
    }

    fn mono_font_name(&self) -> &str {
        primary_font_family(&self.config.mono_font_family, "Cascadia Code")
    }

    /// 创建统一行高配置的 DirectWrite `IDWriteTextFormat`。
    fn create_text_format(
        &self,
        font_family: &str,
        font_size: f32,
        weight: windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT,
        italic: bool,
        line_height_mult: f32,
        align: windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT,
    ) -> windows::core::Result<IDWriteTextFormat> {
        let family_w: Vec<u16> = font_family.encode_utf16().chain(std::iter::once(0)).collect();
        let locale_w = w!("zh-CN");
        let style = if italic {
            DWRITE_FONT_STYLE_ITALIC
        } else {
            DWRITE_FONT_STYLE_NORMAL
        };
        let fmt = unsafe {
            self.dwrite_factory.CreateTextFormat(
                PCWSTR(family_w.as_ptr()),
                None,
                weight,
                style,
                DWRITE_FONT_STRETCH_NORMAL,
                font_size,
                locale_w,
            )?
        };
        unsafe {
            let _ = fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
            let _ = fmt.SetReadingDirection(DWRITE_READING_DIRECTION_LEFT_TO_RIGHT);
            let _ = fmt.SetTextAlignment(align);
            let line_spacing = (font_size * line_height_mult).max(font_size * 1.15);
            let baseline = font_size * 0.82 + (line_spacing - font_size) * 0.38;
            let _ = fmt.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, line_spacing, baseline);
        }
        Ok(fmt)
    }

    /// 为指定富文本片段序列构建设备无关的 `IDWriteTextLayout`。
    fn build_spans_layout(
        &self,
        spans: &[RichSpan],
        base_font: &str,
        base_size: f32,
        base_weight: windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT,
        line_height_mult: f32,
        max_w: f32,
        align: windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT,
        is_code_block: bool,
    ) -> windows::core::Result<(IDWriteTextLayout, FlattenedSpans)> {
        let flat = flatten_spans(spans, is_code_block);
        let fmt = self.create_text_format(
            base_font,
            base_size,
            base_weight,
            false,
            line_height_mult,
            align,
        )?;
        let wide: Vec<u16> = flat.text.encode_utf16().collect();
        let layout = unsafe {
            self.dwrite_factory
                .CreateTextLayout(&wide, &fmt, max_w.max(24.0), 1_000_000.0)?
        };

        let mono_w: Vec<u16> = self
            .mono_font_name()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        for r in &flat.ranges {
            if r.len == 0 {
                continue;
            }
            let range = DWRITE_TEXT_RANGE {
                startPosition: r.start,
                length: r.len,
            };
            unsafe {
                if r.bold {
                    let _ = layout.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, range);
                }
                if r.italic {
                    let _ = layout.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, range);
                }
                if r.strike {
                    let _ = layout.SetStrikethrough(true, range);
                }
                if r.link_url.is_some() {
                    let _ = layout.SetUnderline(true, range);
                }
                if r.code && !is_code_block {
                    let _ = layout.SetFontFamilyName(PCWSTR(mono_w.as_ptr()), range);
                    let _ = layout.SetFontSize((base_size * 0.90).max(11.0), range);
                }
            }
        }

        Ok((layout, flat))
    }

    /// 计算单个普通块的样式参数 `(font, size, weight, line_height, x_offset, extra_pad_x, top_margin, bottom_margin)`。
    fn block_style(
        &self,
        kind: &BlockKind,
        quote_depth: u16,
    ) -> (
        &str,
        f32,
        windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT,
        f32,
        f32,
        f32,
        f32,
        f32,
    ) {
        let base = self.config.font_size;
        let lh = self.config.line_height;
        match kind {
            BlockKind::Heading { level } => {
                let (scale, top_m, bot_m) = match level {
                    1 => (1.92, 24.0, 14.0),
                    2 => (1.50, 22.0, 12.0),
                    3 => (1.26, 18.0, 10.0),
                    4 => (1.10, 15.0, 10.0),
                    5 => (1.00, 13.0, 10.0),
                    _ => (0.94, 12.0, 11.0),
                };
                (
                    self.body_font_name(),
                    base * scale,
                    DWRITE_FONT_WEIGHT_BOLD,
                    1.32,
                    0.0,
                    0.0,
                    top_m,
                    bot_m,
                )
            }
            BlockKind::Paragraph => (
                self.body_font_name(),
                base,
                DWRITE_FONT_WEIGHT_NORMAL,
                lh,
                0.0,
                0.0,
                0.0,
                13.0,
            ),
            BlockKind::CodeBlock { .. } => (
                self.mono_font_name(),
                (base * 0.88).max(12.0),
                DWRITE_FONT_WEIGHT_NORMAL,
                1.52,
                16.0,
                16.0,
                6.0,
                16.0,
            ),
            BlockKind::BlockQuote => {
                let qd = quote_depth.max(1).min(6);
                let nest_indent = f32::from(qd - 1) * 18.0;
                (
                    self.body_font_name(),
                    base,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    lh,
                    nest_indent + 20.0,
                    14.0,
                    4.0,
                    12.0,
                )
            }
            BlockKind::ListItem { depth, task, .. } => {
                let indent = f32::from(*depth) * 22.0 + if task.is_some() { 26.0 } else { 22.0 };
                (
                    self.body_font_name(),
                    base,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    lh,
                    indent,
                    0.0,
                    2.0,
                    6.0,
                )
            }
            BlockKind::Table { .. }
            | BlockKind::ThematicBreak
            | BlockKind::Image { .. } => (
                self.body_font_name(),
                base,
                DWRITE_FONT_WEIGHT_NORMAL,
                lh,
                0.0,
                0.0,
                8.0,
                16.0,
            ),
        }
    }

    /// 依据当前版心宽度重新计算文档所有块的轻量几何索引（不长期持有全部 TextLayout）。
    fn rebuild_layout_metrics_if_needed(&mut self) {
        if !self.layout_dirty {
            return;
        }
        self.layout_dirty = false;
        self.metrics.clear();
        self.table_metrics.clear();
        self.layout_cache.clear();

        let (left, content_w, top_pad) = self.page_geometry();
        let mut cursor_y = top_pad;

        let block_count = self.doc.blocks.len();
        self.metrics.reserve(block_count);

        for idx in 0..block_count {
            let kind = self.doc.blocks[idx].kind.clone();
            match &kind {
                BlockKind::ThematicBreak => {
                    let y_top = cursor_y + 12.0;
                    let height = 16.0;
                    self.metrics.push(BlockMetric {
                        y_top,
                        height,
                        text_x: left,
                        text_y: y_top,
                        text_w: content_w,
                        utf16_len: 0,
                    });
                    cursor_y = y_top + height + 12.0;
                }
                BlockKind::Image { url, alt } => {
                    let y_top = cursor_y + 8.0;
                    let img_h = self.measure_image_block_height(url, alt, content_w);
                    self.metrics.push(BlockMetric {
                        y_top,
                        height: img_h,
                        text_x: left,
                        text_y: y_top,
                        text_w: content_w,
                        utf16_len: alt.encode_utf16().count() as u32,
                    });
                    cursor_y = y_top + img_h + 16.0;
                }
                BlockKind::Table { alignments, rows } => {
                    let y_top = cursor_y + 8.0;
                    let (tmetric, total_h) = self.measure_table(alignments, rows, left, content_w);
                    self.table_metrics.insert(idx, tmetric);
                    self.metrics.push(BlockMetric {
                        y_top,
                        height: total_h,
                        text_x: left,
                        text_y: y_top,
                        text_w: content_w,
                        utf16_len: 0,
                    });
                    cursor_y = y_top + total_h + 16.0;
                }
                _ => {
                    let qd = self.doc.blocks[idx].quote_depth;
                    let (font, size, weight, lh, x_off, pad_r, top_m, bot_m) =
                        self.block_style(&kind, qd);
                    // 在不同类型列表（如无序切有序、任务列表切普通列表）之间增加组间呼吸空隙
                    let list_group_gap = if idx > 0 {
                        match (&self.doc.blocks[idx - 1].kind, &kind) {
                            (
                                BlockKind::ListItem {
                                    ordered: prev_ord,
                                    task: prev_task,
                                    ..
                                },
                                BlockKind::ListItem {
                                    ordered: cur_ord,
                                    task: cur_task,
                                    depth: 0,
                                    ..
                                },
                            ) if prev_ord != cur_ord || prev_task.is_some() != cur_task.is_some() => {
                                10.0
                            }
                            _ => 0.0,
                        }
                    } else {
                        0.0
                    };
                    let font_owned = font.to_string();
                    let is_code = matches!(kind, BlockKind::CodeBlock { .. });
                    let is_quote = matches!(kind, BlockKind::BlockQuote);
                    let heading_border = matches!(kind, BlockKind::Heading { level: 1 | 2 });

                    let text_x = left + x_off;
                    let text_w = (content_w - x_off - pad_r).max(80.0);
                    let y_top = if idx == 0 {
                        cursor_y
                    } else {
                        cursor_y + top_m + list_group_gap
                    };
                    let inner_top = if is_code {
                        if matches!(kind, BlockKind::CodeBlock { language: Some(_) }) {
                            28.0
                        } else {
                            14.0
                        }
                    } else if is_quote {
                        10.0
                    } else {
                        0.0
                    };
                    let inner_bot = if is_code {
                        14.0
                    } else if is_quote {
                        10.0
                    } else if heading_border {
                        8.0
                    } else {
                        0.0
                    };

                    let (text_h, utf16_len) = match self.build_spans_layout(
                        &self.doc.blocks[idx].spans,
                        &font_owned,
                        size,
                        weight,
                        lh,
                        text_w,
                        DWRITE_TEXT_ALIGNMENT_LEADING,
                        is_code,
                    ) {
                        Ok((layout, flat)) => {
                            let mut tm = Default::default();
                            let h = if unsafe { layout.GetMetrics(&mut tm) }.is_ok() {
                                tm.height.max(size * lh)
                            } else {
                                size * lh
                            };
                            (h, flat.utf16_len)
                        }
                        Err(_) => (size * lh, 0),
                    };

                    let height = inner_top + text_h + inner_bot;
                    self.metrics.push(BlockMetric {
                        y_top,
                        height,
                        text_x,
                        text_y: y_top + inner_top,
                        text_w,
                        utf16_len,
                    });
                    cursor_y = y_top + height + bot_m;
                }
            }
        }

        self.content_total_h = (cursor_y + 48.0).max(self.client_h);
        self.clamp_scroll();
    }

    /// 计算表格的列宽、行高与总高度，严格限制总宽不超过版心 `content_w`。
    fn measure_table(
        &self,
        alignments: &[TableAlign],
        rows: &[Vec<Vec<RichSpan>>],
        table_x: f32,
        content_w: f32,
    ) -> (TableMetric, f32) {
        let col_count = alignments
            .len()
            .max(rows.iter().map(Vec::len).max().unwrap_or(0))
            .max(1);

        // 按各列内容最长字符估算权重，保证不撑破版心
        let mut col_weights = vec![4.0f32; col_count];
        for row in rows {
            for (c, cell) in row.iter().enumerate().take(col_count) {
                let len: usize = cell.iter().map(|s| s.text.chars().count()).sum();
                col_weights[c] = col_weights[c].max((len as f32).clamp(4.0, 36.0));
            }
        }
        let weight_sum: f32 = col_weights.iter().sum::<f32>().max(1.0);
        let min_col = (content_w / (col_count as f32 * 2.2)).clamp(52.0, 96.0);
        let mut col_widths: Vec<f32> = col_weights
            .iter()
            .map(|&w| ((w / weight_sum) * content_w).max(min_col))
            .collect();
        let raw_sum: f32 = col_widths.iter().sum();
        if raw_sum > content_w && raw_sum > 0.0 {
            let ratio = content_w / raw_sum;
            for w in &mut col_widths {
                *w *= ratio;
            }
        }

        let cell_pad_x = 10.0;
        let cell_pad_y = 8.0;
        let cell_font_size = (self.config.font_size * 0.92).max(12.0);
        let mut row_heights = Vec::with_capacity(rows.len());
        let mut total_h = 0.0f32;

        for (r_idx, row) in rows.iter().enumerate() {
            let mut max_h = cell_font_size * 1.45;
            let weight = if r_idx == 0 {
                DWRITE_FONT_WEIGHT_SEMI_BOLD
            } else {
                DWRITE_FONT_WEIGHT_NORMAL
            };
            for c_idx in 0..col_count {
                let cell_spans = row.get(c_idx).map(Vec::as_slice).unwrap_or(&[]);
                let align = table_align_to_dwrite(
                    alignments.get(c_idx).copied().unwrap_or(TableAlign::None),
                );
                let inner_w = (col_widths[c_idx] - cell_pad_x * 2.0).max(24.0);
                if let Ok((layout, _)) = self.build_spans_layout(
                    cell_spans,
                    self.body_font_name(),
                    cell_font_size,
                    weight,
                    1.45,
                    inner_w,
                    align,
                    false,
                ) {
                    let mut tm = Default::default();
                    if unsafe { layout.GetMetrics(&mut tm) }.is_ok() {
                        max_h = max_h.max(tm.height);
                    }
                }
            }
            let rh = max_h + cell_pad_y * 2.0;
            row_heights.push(rh);
            total_h += rh;
        }

        (
            TableMetric {
                col_widths,
                row_heights,
                table_x,
                table_w: content_w,
            },
            total_h.max(32.0),
        )
    }

    /// 懒加载并解码本地图片，返回其在版心中的等比显示高度；远程 URL 或损坏图片返回安全占位高度。
    fn measure_image_block_height(&mut self, url: &str, alt: &str, content_w: f32) -> f32 {
        self.ensure_image_loaded(url);
        let caption_h = if alt.trim().is_empty() { 0.0 } else { 24.0 };
        match self.image_cache.get(url) {
            Some(CachedImage::Loaded { width, height, .. }) if *width > 0 && *height > 0 => {
                let scale = (content_w / *width as f32).min(1.0);
                let disp_h = (*height as f32 * scale).clamp(24.0, 640.0);
                disp_h + caption_h
            }
            _ => 56.0,
        }
    }

    fn ensure_image_loaded(&mut self, url: &str) {
        self.image_tick = self.image_tick.wrapping_add(1);
        let tick = self.image_tick;
        if let Some(entry) = self.image_cache.get_mut(url) {
            match entry {
                CachedImage::Loaded { last_used, .. } | CachedImage::Failed { last_used, .. } => {
                    *last_used = tick;
                }
            }
            return;
        }

        // 超出上限时淘汰最久未用的图片缓存
        if self.image_cache.len() >= MAX_CACHED_IMAGES
            && let Some(oldest_key) = self
                .image_cache
                .iter()
                .min_by_key(|(_, v)| match v {
                    CachedImage::Loaded { last_used, .. }
                    | CachedImage::Failed { last_used, .. } => *last_used,
                })
                .map(|(k, _)| k.clone())
        {
            self.image_cache.remove(&oldest_key);
        }

        let trimmed = url.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("data:")
            || lower.starts_with("javascript:")
        {
            self.image_cache.insert(
                url.to_string(),
                CachedImage::Failed {
                    reason: format!("外链图片未自动联网加载: {trimmed}"),
                    last_used: tick,
                },
            );
            return;
        }

        let resolved = resolve_local_image_path(trimmed, self.doc_dir.as_deref());
        let Some(img_path) = resolved else {
            self.image_cache.insert(
                url.to_string(),
                CachedImage::Failed {
                    reason: format!("图片路径无效: {trimmed}"),
                    last_used: tick,
                },
            );
            return;
        };

        if !img_path.is_file() {
            self.image_cache.insert(
                url.to_string(),
                CachedImage::Failed {
                    reason: format!("找不到本地图片: {trimmed}"),
                    last_used: tick,
                },
            );
            return;
        }

        match self.decode_wic_image(&img_path) {
            Ok((width, height, pixels)) => {
                self.image_cache.insert(
                    url.to_string(),
                    CachedImage::Loaded {
                        width,
                        height,
                        pixels,
                        last_used: tick,
                    },
                );
            }
            Err(_) => {
                self.image_cache.insert(
                    url.to_string(),
                    CachedImage::Failed {
                        reason: format!("图片解码失败或文件已损坏: {trimmed}"),
                        last_used: tick,
                    },
                );
            }
        }
    }

    /// 使用 WIC 将本地图片解码为 32bpp 预乘 BGRA 内存缓冲区（超宽图片自动缩放控制内存）。
    fn decode_wic_image(&mut self, path: &Path) -> windows::core::Result<(u32, u32, Vec<u8>)> {
        if self.wic_factory.is_none() {
            let factory: IWICImagingFactory =
                unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };
            self.wic_factory = Some(factory);
        }
        let wic = self.wic_factory.as_ref().unwrap();
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let decoder = unsafe {
            wic.CreateDecoderFromFilename(
                PCWSTR(wide.as_ptr()),
                None,
                windows::Win32::Foundation::GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )?
        };
        let frame = unsafe { decoder.GetFrame(0)? };
        let mut src_w = 0u32;
        let mut src_h = 0u32;
        unsafe { frame.GetSize(&mut src_w, &mut src_h)? };
        if src_w == 0 || src_h == 0 || src_w > 16384 || src_h > 16384 {
            return Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_FAIL,
            ));
        }

        let (dst_w, dst_h) = if src_w > MAX_IMAGE_DECODE_DIM || src_h > MAX_IMAGE_DECODE_DIM {
            let scale = (MAX_IMAGE_DECODE_DIM as f64 / src_w as f64)
                .min(MAX_IMAGE_DECODE_DIM as f64 / src_h as f64);
            (
                ((src_w as f64 * scale).round() as u32).max(1),
                ((src_h as f64 * scale).round() as u32).max(1),
            )
        } else {
            (src_w, src_h)
        };

        let converter = unsafe { wic.CreateFormatConverter()? };
        if dst_w != src_w || dst_h != src_h {
            let scaler = unsafe { wic.CreateBitmapScaler()? };
            unsafe {
                scaler.Initialize(
                    &frame,
                    dst_w,
                    dst_h,
                    windows::Win32::Graphics::Imaging::WICBitmapInterpolationModeFant,
                )?;
                converter.Initialize(
                    &scaler,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )?;
            }
        } else {
            unsafe {
                converter.Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )?;
            }
        }

        let stride = dst_w.checked_mul(4).unwrap_or(0);
        let byte_len = (stride as usize).checked_mul(dst_h as usize).unwrap_or(0);
        if byte_len == 0 || byte_len > 32 * 1024 * 1024 {
            return Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_OUTOFMEMORY,
            ));
        }
        let mut pixels = vec![0u8; byte_len];
        unsafe {
            converter.CopyPixels(std::ptr::null(), stride, &mut pixels)?;
        }
        Ok((dst_w, dst_h, pixels))
    }

    /// 获取或按需构建指定块的 `IDWriteTextLayout`（受有界 LRU 容量控制）。
    fn get_or_build_block_layout(&mut self, idx: usize) -> Option<&CachedLayoutEntry> {
        self.cache_tick = self.cache_tick.wrapping_add(1);
        let tick = self.cache_tick;

        if let Some(entry) = self.layout_cache.get_mut(&idx) {
            entry.last_used = tick;
            return self.layout_cache.get(&idx);
        }

        if idx >= self.doc.blocks.len() || idx >= self.metrics.len() {
            return None;
        }

        // 超出 LRU 上限时淘汰最久未用项
        if self.layout_cache.len() >= MAX_CACHED_LAYOUTS
            && let Some(oldest_idx) = self
                .layout_cache
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(&k, _)| k)
        {
            self.layout_cache.remove(&oldest_idx);
        }

        let kind = self.doc.blocks[idx].kind.clone();
        if matches!(
            kind,
            BlockKind::Table { .. } | BlockKind::ThematicBreak | BlockKind::Image { .. }
        ) {
            return None;
        }

        let qd = self.doc.blocks[idx].quote_depth;
        let (font, size, weight, lh, _, _, _, _) = self.block_style(&kind, qd);
        let font_owned = font.to_string();
        let is_code = matches!(kind, BlockKind::CodeBlock { .. });
        let text_w = self.metrics[idx].text_w;

        let (layout, flat) = self
            .build_spans_layout(
                &self.doc.blocks[idx].spans,
                &font_owned,
                size,
                weight,
                lh,
                text_w,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                is_code,
            )
            .ok()?;

        self.layout_cache.insert(
            idx,
            CachedLayoutEntry {
                layout,
                ranges: flat.ranges,
                last_used: tick,
            },
        );
        self.layout_cache.get(&idx)
    }

    /// 确保软件 `ID2D1DCRenderTarget`（`D2D1_RENDER_TARGET_TYPE_SOFTWARE` + `ALPHA_MODE_IGNORE`）已创建。
    ///
    /// 使用软件渲染目标避免加载数十 MB GPU 3D 驱动工作集，同时 100% 保留 Direct2D 抗锯齿矢量与
    /// DirectWrite ClearType 文本渲染，并使屏幕 `WM_PAINT` 与离屏 `WM_PRINTCLIENT` 像素完全同源。
    fn ensure_dc_render_target(&mut self) -> windows::core::Result<ID2D1DCRenderTarget> {
        let dpi = self.dpi_scale * 96.0;
        if let Some(rt) = &self.dc_rt {
            unsafe { rt.SetDpi(dpi, dpi) };
            return Ok(rt.clone());
        }

        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: dpi,
            dpiY: dpi,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: Default::default(),
        };
        let rt = unsafe { self.d2d_factory.CreateDCRenderTarget(&props)? };
        self.dc_rt = Some(rt.clone());
        Ok(rt)
    }

    /// 将当前完整 Direct2D 场景渲染至双缓冲 DIBSection 并 `BitBlt` 到指定 `HDC`。
    fn paint_to_hdc(&mut self, target_hdc: HDC) {
        if target_hdc.is_invalid() || target_hdc.0.is_null() {
            return;
        }
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        let pw = rc.right - rc.left;
        let ph = rc.bottom - rc.top;
        if pw <= 0 || ph <= 0 {
            return;
        }

        self.rebuild_layout_metrics_if_needed();

        let need_realloc = match &self.back_buffer {
            Some(bb) => bb.width != pw || bb.height != ph,
            None => true,
        };
        if need_realloc {
            self.back_buffer = SoftwareBackBuffer::new(pw, ph);
        }

        let Some(mem_dc) = self.back_buffer.as_ref().map(|b| b.mem_dc) else {
            return;
        };

        if let Ok(dc_rt) = self.ensure_dc_render_target() {
            let bind_rc = RECT {
                left: 0,
                top: 0,
                right: pw,
                bottom: ph,
            };
            if unsafe { dc_rt.BindDC(mem_dc, &bind_rc) }.is_ok() {
                let rt: ID2D1RenderTarget = dc_rt.cast().unwrap();
                let res = self.render_scene(&rt);
                if res.is_err() {
                    self.dc_rt = None;
                }
            }
            unsafe {
                let _ = BitBlt(target_hdc, 0, 0, pw, ph, Some(mem_dc), 0, 0, SRCCOPY);
            }
        }
    }

    /// 执行标准屏幕 `WM_PAINT` 绘制。
    fn on_paint(&mut self) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = unsafe { BeginPaint(self.hwnd, &mut ps) };
        self.paint_to_hdc(hdc);
        unsafe {
            let _ = EndPaint(self.hwnd, &ps);
        }
    }

    /// 响应 `WM_PRINT` / `WM_PRINTCLIENT`，与屏幕绘制复走同一双缓冲管线，彻底杜绝截屏白屏。
    fn on_print_client(&mut self, hdc: HDC) {
        self.paint_to_hdc(hdc);
    }

    /// 统一 Direct2D 场景渲染管线（同时服务屏幕绘制与 `WM_PRINTCLIENT` 离屏截图）。
    fn render_scene(&mut self, rt: &ID2D1RenderTarget) -> windows::core::Result<()> {
        let brushes = FrameBrushes::new(rt, &self.palette)?;
        unsafe {
            rt.BeginDraw();
            rt.SetTransform(&Matrix3x2::identity());
            rt.Clear(Some(&self.palette.background.to_d2d(1.0)));
        }

        let draw_res = self.render_scene_inner(rt, &brushes);

        let end_res = unsafe { rt.EndDraw(None, None) };
        draw_res.and(end_res)
    }

    fn render_scene_inner(
        &mut self,
        rt: &ID2D1RenderTarget,
        brushes: &FrameBrushes,
    ) -> windows::core::Result<()> {
        // 1. 错误提示横幅状态
        if let Some(err_msg) = self.error_banner.clone() {
            self.render_banner_card(rt, brushes, "无法打开文档", &err_msg, true)?;
            return Ok(());
        }

        // 2. 无参启动极简欢迎状态
        if self.is_empty_welcome && self.doc.blocks.is_empty() {
            self.render_banner_card(
                rt,
                brushes,
                "MDReader 纯粹阅读模式",
                "点击左上角「打开」按钮、按 Ctrl+O 或直接拖入 .md 文件即可开始阅读。\n打开文件后按 Ctrl+E / F2 可一键切换至修改模式，按 Ctrl+S 保存后自动回到阅读模式。",
                false,
            )?;
            return Ok(());
        }

        // 3. 二分查找首个与当前视口 [scroll_y, scroll_y + client_h] 相交的语义块
        let view_top = self.scroll_y;
        let view_bot = self.scroll_y + self.client_h;
        let first_idx = self
            .metrics
            .partition_point(|m| m.y_top + m.height < view_top - 40.0);

        let (sel_start, sel_end) = self.normalized_selection();
        let active_match_idx = self.active_match;

        for idx in first_idx..self.doc.blocks.len() {
            let metric = self.metrics[idx].clone();
            if metric.y_top > view_bot + 40.0 {
                break;
            }
            let block_kind = self.doc.blocks[idx].kind.clone();
            match block_kind {
                BlockKind::ThematicBreak => {
                    let y = metric.y_top + metric.height * 0.5 - self.scroll_y;
                    unsafe {
                        rt.DrawLine(
                            Vector2 {
                                X: metric.text_x,
                                Y: y,
                            },
                            Vector2 {
                                X: metric.text_x + metric.text_w,
                                Y: y,
                            },
                            &brushes.border,
                            1.0,
                            None,
                        );
                    }
                }
                BlockKind::Image { url, alt } => {
                    self.render_image_block(rt, brushes, &metric, &url, &alt);
                }
                BlockKind::Table { alignments, rows } => {
                    if let Some(tmetric) = self.table_metrics.get(&idx).cloned() {
                        self.render_table_block(
                            rt,
                            brushes,
                            idx,
                            &metric,
                            &tmetric,
                            &alignments,
                            &rows,
                            active_match_idx,
                        );
                    }
                }
                _ => {
                    self.render_text_block(
                        rt,
                        brushes,
                        idx,
                        &block_kind,
                        &metric,
                        sel_start,
                        sel_end,
                        active_match_idx,
                    );
                }
            }
        }

        // 4. 绘制右侧极简细滚动指示条（无额外控件噪音）
        if self.content_total_h > self.client_h + 1.0 && self.client_h > 40.0 {
            let ratio = (self.client_h / self.content_total_h).clamp(0.06, 1.0);
            let thumb_h = (self.client_h * ratio).max(28.0);
            let max_scroll = (self.content_total_h - self.client_h).max(1.0);
            let thumb_y = (self.scroll_y / max_scroll) * (self.client_h - thumb_h - 8.0) + 4.0;
            let bar_x = self.client_w - 9.0;
            let rr = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: bar_x,
                    top: thumb_y,
                    right: bar_x + 5.0,
                    bottom: thumb_y + thumb_h,
                },
                radiusX: 2.5,
                radiusY: 2.5,
            };
            unsafe {
                rt.FillRoundedRectangle(&rr, &brushes.scrollbar);
            }
        }

        Ok(())
    }

    /// 渲染空状态引导或错误提示居中卡片（动态测量文字高度，任意 DPI 与窗口宽度下绝不破框）。
    fn render_banner_card(
        &self,
        rt: &ID2D1RenderTarget,
        brushes: &FrameBrushes,
        title: &str,
        body: &str,
        is_error: bool,
    ) -> windows::core::Result<()> {
        let (left, content_w, top_pad) = self.page_geometry();
        let card_w = content_w.min(580.0).max(260.0);
        let card_x = left + (content_w - card_w) * 0.5;
        let card_y = (top_pad + 36.0).min(110.0);
        let pad_x = 24.0;
        let pad_top = 22.0;
        let pad_bot = 24.0;
        let gap = 12.0;
        let inner_w = (card_w - pad_x * 2.0).max(160.0);

        let title_fmt = self.create_text_format(
            self.body_font_name(),
            (self.config.font_size * 1.25).clamp(18.0, 24.0),
            DWRITE_FONT_WEIGHT_BOLD,
            false,
            1.35,
            DWRITE_TEXT_ALIGNMENT_LEADING,
        )?;
        let title_w: Vec<u16> = title.encode_utf16().collect();
        let title_layout = unsafe {
            self.dwrite_factory
                .CreateTextLayout(&title_w, &title_fmt, inner_w, 400.0)?
        };
        let mut title_m = Default::default();
        let title_h = if unsafe { title_layout.GetMetrics(&mut title_m) }.is_ok() {
            title_m.height.max(24.0)
        } else {
            28.0
        };

        let body_fmt = self.create_text_format(
            self.body_font_name(),
            self.config.font_size.clamp(13.0, 18.0),
            DWRITE_FONT_WEIGHT_NORMAL,
            false,
            1.65,
            DWRITE_TEXT_ALIGNMENT_LEADING,
        )?;
        let body_w: Vec<u16> = body.encode_utf16().collect();
        let body_layout = unsafe {
            self.dwrite_factory
                .CreateTextLayout(&body_w, &body_fmt, inner_w, 1000.0)?
        };
        let mut body_m = Default::default();
        let body_h = if unsafe { body_layout.GetMetrics(&mut body_m) }.is_ok() {
            body_m.height.max(24.0)
        } else {
            64.0
        };

        let card_h = pad_top + title_h + gap + body_h + pad_bot;
        let rr = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: card_x,
                top: card_y,
                right: card_x + card_w,
                bottom: card_y + card_h,
            },
            radiusX: 8.0,
            radiusY: 8.0,
        };
        unsafe {
            rt.FillRoundedRectangle(&rr, &brushes.code_bg);
            rt.DrawRoundedRectangle(&rr, &brushes.code_border, 1.0, None);
        }

        let title_brush = if is_error {
            &brushes.error_text
        } else {
            &brushes.heading
        };
        unsafe {
            rt.DrawTextLayout(
                Vector2 {
                    X: card_x + pad_x,
                    Y: card_y + pad_top,
                },
                &title_layout,
                title_brush,
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            );
            rt.DrawTextLayout(
                Vector2 {
                    X: card_x + pad_x,
                    Y: card_y + pad_top + title_h + gap,
                },
                &body_layout,
                &brushes.muted,
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            );
        }
        Ok(())
    }

    /// 渲染单个普通文本语义块（标题、段落、列表/任务列表、引用、围栏代码块）。
    #[allow(clippy::too_many_arguments)]
    fn render_text_block(
        &mut self,
        rt: &ID2D1RenderTarget,
        brushes: &FrameBrushes,
        idx: usize,
        kind: &BlockKind,
        metric: &BlockMetric,
        sel_start: Option<DocPos>,
        sel_end: Option<DocPos>,
        active_match_idx: Option<usize>,
    ) {
        let (left, content_w, _) = self.page_geometry();
        let block_top = metric.y_top - self.scroll_y;
        let text_top = metric.text_y - self.scroll_y;

        // A. 绘制块级背景与装饰元素
        match kind {
            BlockKind::CodeBlock { language } => {
                let rr = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left,
                        top: block_top,
                        right: left + content_w,
                        bottom: block_top + metric.height,
                    },
                    radiusX: 6.0,
                    radiusY: 6.0,
                };
                unsafe {
                    rt.FillRoundedRectangle(&rr, &brushes.code_bg);
                    rt.DrawRoundedRectangle(&rr, &brushes.code_border, 1.0, None);
                }
                if let Some(lang) = language.as_deref().filter(|s| !s.trim().is_empty())
                    && let Ok(lfmt) = self.create_text_format(
                        self.mono_font_name(),
                        11.5,
                        DWRITE_FONT_WEIGHT_SEMI_BOLD,
                        false,
                        1.2,
                        DWRITE_TEXT_ALIGNMENT_TRAILING,
                    )
                {
                    let lw: Vec<u16> = lang.encode_utf16().collect();
                    if let Ok(llayout) = unsafe {
                        self.dwrite_factory
                            .CreateTextLayout(&lw, &lfmt, (content_w - 28.0).max(60.0), 20.0)
                    } {
                        unsafe {
                            rt.DrawTextLayout(
                                Vector2 {
                                    X: left + 14.0,
                                    Y: block_top + 7.0,
                                },
                                &llayout,
                                &brushes.muted,
                                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                            );
                        }
                    }
                }
            }
            BlockKind::BlockQuote => {
                let qd = self.doc.blocks[idx].quote_depth.max(1).min(6);
                let nest_indent = f32::from(qd - 1) * 18.0;
                let bg_left = left + nest_indent;
                let rr = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: bg_left,
                        top: block_top,
                        right: left + content_w,
                        bottom: block_top + metric.height,
                    },
                    radiusX: 4.0,
                    radiusY: 4.0,
                };
                unsafe {
                    rt.FillRoundedRectangle(&rr, &brushes.quote_bg);
                    // 按嵌套深度逐层绘制递进竖线，清晰展现多级引用层次
                    for level in 0..qd {
                        let bar_x = left + f32::from(level) * 18.0;
                        let bar_w = if level + 1 == qd { 3.5 } else { 2.5 };
                        let bar = D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F {
                                left: bar_x,
                                top: block_top,
                                right: bar_x + bar_w,
                                bottom: block_top + metric.height,
                            },
                            radiusX: 1.5,
                            radiusY: 1.5,
                        };
                        let bar_brush = if level + 1 == qd {
                            &brushes.quote_bar
                        } else {
                            &brushes.border
                        };
                        rt.FillRoundedRectangle(&bar, bar_brush);
                    }
                }
            }
            BlockKind::Heading { level: 1 | 2 } => {
                let line_y = block_top + metric.height - 2.0;
                unsafe {
                    rt.DrawLine(
                        Vector2 { X: left, Y: line_y },
                        Vector2 {
                            X: left + content_w,
                            Y: line_y,
                        },
                        &brushes.border,
                        1.0,
                        None,
                    );
                }
            }
            BlockKind::ListItem {
                ordered,
                index,
                depth,
                task,
            } => {
                let base_x = left + f32::from(*depth) * 22.0;
                let mid_y = text_top + self.config.font_size * self.config.line_height * 0.48;
                if let Some(checked) = task {
                    let bx = base_x + 4.0;
                    let by = mid_y - 7.0;
                    let rr = D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: bx,
                            top: by,
                            right: bx + 14.0,
                            bottom: by + 14.0,
                        },
                        radiusX: 3.0,
                        radiusY: 3.0,
                    };
                    unsafe {
                        if *checked {
                            rt.FillRoundedRectangle(&rr, &brushes.link);
                            rt.DrawLine(
                                Vector2 {
                                    X: bx + 3.2,
                                    Y: by + 7.2,
                                },
                                Vector2 {
                                    X: bx + 6.0,
                                    Y: by + 10.2,
                                },
                                &brushes.check_mark,
                                1.7,
                                None,
                            );
                            rt.DrawLine(
                                Vector2 {
                                    X: bx + 5.8,
                                    Y: by + 10.2,
                                },
                                Vector2 {
                                    X: bx + 10.8,
                                    Y: by + 4.0,
                                },
                                &brushes.check_mark,
                                1.7,
                                None,
                            );
                        } else {
                            rt.DrawRoundedRectangle(&rr, &brushes.border, 1.4, None);
                        }
                    }
                } else if *ordered {
                    let label = format!("{}.", index.unwrap_or(1));
                    if let Ok(nfmt) = self.create_text_format(
                        self.body_font_name(),
                        self.config.font_size * 0.94,
                        DWRITE_FONT_WEIGHT_NORMAL,
                        false,
                        self.config.line_height,
                        DWRITE_TEXT_ALIGNMENT_TRAILING,
                    ) {
                        let nw: Vec<u16> = label.encode_utf16().collect();
                        if let Ok(nlayout) =
                            unsafe { self.dwrite_factory.CreateTextLayout(&nw, &nfmt, 18.0, 32.0) }
                        {
                            unsafe {
                                rt.DrawTextLayout(
                                    Vector2 {
                                        X: base_x,
                                        Y: text_top,
                                    },
                                    &nlayout,
                                    &brushes.muted,
                                    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                                );
                            }
                        }
                    }
                } else {
                    let cx = base_x + 10.0;
                    let ellipse = windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                        point: Vector2 { X: cx, Y: mid_y },
                        radiusX: 2.6,
                        radiusY: 2.6,
                    };
                    unsafe {
                        if depth % 2 == 0 {
                            rt.FillEllipse(&ellipse, &brushes.muted);
                        } else {
                            rt.DrawEllipse(&ellipse, &brushes.muted, 1.2, None);
                        }
                    }
                }
            }
            _ => {}
        }

        // B. 从 LRU 缓存获取 TextLayout，绘制行内代码胶囊底色、查找高亮、划词选区与多色文本
        let is_code_block = matches!(kind, BlockKind::CodeBlock { .. });
        let is_heading = matches!(kind, BlockKind::Heading { .. });
        let is_h6 = matches!(kind, BlockKind::Heading { level: 6 });
        let is_quote = matches!(kind, BlockKind::BlockQuote);
        let is_completed_task = matches!(kind, BlockKind::ListItem { task: Some(true), .. });

        // 先克隆本块查找匹配列表，避免借用冲突
        let block_matches: Vec<(u32, u32, bool)> = self
            .find_matches
            .iter()
            .enumerate()
            .filter(|(_, m)| m.block == idx && m.cell_idx.is_none())
            .map(|(mi, m)| (m.utf16_start, m.utf16_len, Some(mi) == active_match_idx))
            .collect();

        let Some(entry) = self.get_or_build_block_layout(idx) else {
            return;
        };
        let layout = entry.layout.clone();
        let ranges = entry.ranges.clone();
        let utf16_len = metric.utf16_len;

        // 1) 行内代码圆角胶囊底色（非围栏代码块，依赖薄空格自然隔离相邻汉字/标点，零穿模）
        if !is_code_block {
            for r in &ranges {
                if r.code && r.len > 0 {
                    draw_text_range_rects(
                        rt,
                        &layout,
                        metric.text_x,
                        text_top,
                        r.start,
                        r.len,
                        &brushes.code_bg,
                        Some(&brushes.code_border),
                        0.5,
                        3.5,
                    );
                }
            }
        }

        // 2) 页内查找高亮
        for (start, len, is_active) in block_matches {
            let br = if is_active {
                &brushes.find_active
            } else {
                &brushes.find_match
            };
            draw_text_range_rects(rt, &layout, metric.text_x, text_top, start, len, br, None, 0.5, 2.0);
        }

        // 3) 跨块划词选区高亮
        if let (Some(a), Some(b)) = (sel_start, sel_end)
            && idx >= a.block
            && idx <= b.block
            && utf16_len > 0
        {
            let s0 = if idx == a.block { a.utf16_offset } else { 0 };
            let s1 = if idx == b.block {
                b.utf16_offset
            } else {
                utf16_len
            };
            if s1 > s0 {
                draw_text_range_rects(
                    rt,
                    &layout,
                    metric.text_x,
                    text_top,
                    s0,
                    s1 - s0,
                    &brushes.selection,
                    None,
                    0.0,
                    1.5,
                );
            }
        }

        // 4) 绑定当前 RenderTarget 的多色 DrawingEffect（超链接、词法高亮等）
        let full_range = DWRITE_TEXT_RANGE {
            startPosition: 0,
            length: utf16_len,
        };
        unsafe {
            let _ = layout.SetDrawingEffect(None, full_range);
        }
        for r in &ranges {
            if r.len == 0 {
                continue;
            }
            let dr = DWRITE_TEXT_RANGE {
                startPosition: r.start,
                length: r.len,
            };
            if let Some(tok) = r.highlight {
                let eff: windows::core::IUnknown = brushes.token_brush(tok).cast().unwrap();
                unsafe {
                    let _ = layout.SetDrawingEffect(&eff, dr);
                }
            } else if r.link_url.is_some() {
                let eff: windows::core::IUnknown = brushes.link.cast().unwrap();
                unsafe {
                    let _ = layout.SetDrawingEffect(&eff, dr);
                }
            }
        }

        let default_brush = if is_h6 || is_quote || is_completed_task {
            &brushes.muted
        } else if is_heading {
            &brushes.heading
        } else if is_code_block {
            &brushes.tok_plain
        } else {
            &brushes.text
        };

        unsafe {
            rt.DrawTextLayout(
                Vector2 {
                    X: metric.text_x,
                    Y: text_top,
                },
                &layout,
                default_brush,
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            );
            // 绘制后立即解绑 DrawingEffect，保证跨 RenderTarget（如 WM_PRINTCLIENT）绝对零资源域冲突
            let _ = layout.SetDrawingEffect(None, full_range);
        }
    }

    /// 渲染 GFM 表格（表头着色、隔行斑马纹、网格边框、对齐排版与单元格查找高亮）。
    #[allow(clippy::too_many_arguments)]
    fn render_table_block(
        &self,
        rt: &ID2D1RenderTarget,
        brushes: &FrameBrushes,
        block_idx: usize,
        metric: &BlockMetric,
        tmetric: &TableMetric,
        alignments: &[TableAlign],
        rows: &[Vec<Vec<RichSpan>>],
        active_match_idx: Option<usize>,
    ) {
        let top0 = metric.y_top - self.scroll_y;
        let cell_pad_x = 10.0;
        let cell_pad_y = 8.0;
        let cell_font_size = (self.config.font_size * 0.92).max(12.0);
        let col_count = tmetric.col_widths.len();

        let mut row_y = top0;
        for (r_idx, row) in rows.iter().enumerate() {
            let rh = tmetric.row_heights.get(r_idx).copied().unwrap_or(32.0);
            let row_rect = D2D_RECT_F {
                left: tmetric.table_x,
                top: row_y,
                right: tmetric.table_x + tmetric.table_w,
                bottom: row_y + rh,
            };

            if r_idx == 0 {
                unsafe { rt.FillRectangle(&row_rect, &brushes.table_header_bg) };
            } else if r_idx % 2 == 0 {
                unsafe { rt.FillRectangle(&row_rect, &brushes.table_stripe_bg) };
            }

            let weight = if r_idx == 0 {
                DWRITE_FONT_WEIGHT_SEMI_BOLD
            } else {
                DWRITE_FONT_WEIGHT_NORMAL
            };

            let mut col_x = tmetric.table_x;
            for c_idx in 0..col_count {
                let cw = tmetric.col_widths[c_idx];
                let cell_rect = D2D_RECT_F {
                    left: col_x,
                    top: row_y,
                    right: col_x + cw,
                    bottom: row_y + rh,
                };
                unsafe {
                    rt.DrawRectangle(&cell_rect, &brushes.border, 1.0, None);
                }

                let cell_spans = row.get(c_idx).map(Vec::as_slice).unwrap_or(&[]);
                let align = table_align_to_dwrite(
                    alignments.get(c_idx).copied().unwrap_or(TableAlign::None),
                );
                let inner_w = (cw - cell_pad_x * 2.0).max(20.0);
                if let Ok((layout, flat)) = self.build_spans_layout(
                    cell_spans,
                    self.body_font_name(),
                    cell_font_size,
                    weight,
                    1.45,
                    inner_w,
                    align,
                    false,
                ) {
                    let tx = col_x + cell_pad_x;
                    let ty = row_y + cell_pad_y;

                    // 行内代码胶囊底色
                    for sp in &flat.ranges {
                        if sp.code && sp.len > 0 {
                            draw_text_range_rects(
                                rt,
                                &layout,
                                tx,
                                ty,
                                sp.start,
                                sp.len,
                                &brushes.code_bg,
                                Some(&brushes.code_border),
                                0.5,
                                3.0,
                            );
                        }
                    }

                    // 表格单元格查找命中高亮
                    for (mi, m) in self.find_matches.iter().enumerate() {
                        if m.block == block_idx && m.cell_idx == Some((r_idx, c_idx)) {
                            let br = if Some(mi) == active_match_idx {
                                &brushes.find_active
                            } else {
                                &brushes.find_match
                            };
                            draw_text_range_rects(
                                rt,
                                &layout,
                                tx,
                                ty,
                                m.utf16_start,
                                m.utf16_len,
                                br,
                                None,
                                0.5,
                                2.0,
                            );
                        }
                    }

                    // 单元格链接着色
                    for sp in &flat.ranges {
                        if sp.link_url.is_some() && sp.len > 0 {
                            let eff: windows::core::IUnknown = brushes.link.cast().unwrap();
                            unsafe {
                                let _ = layout.SetDrawingEffect(
                                    &eff,
                                    DWRITE_TEXT_RANGE {
                                        startPosition: sp.start,
                                        length: sp.len,
                                    },
                                );
                            }
                        }
                    }

                    let br = if r_idx == 0 {
                        &brushes.heading
                    } else {
                        &brushes.text
                    };
                    unsafe {
                        rt.DrawTextLayout(
                            Vector2 { X: tx, Y: ty },
                            &layout,
                            br,
                            D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                        );
                    }
                }
                col_x += cw;
            }
            row_y += rh;
        }
    }

    /// 渲染本地图片（带缩放边框与图注）或破图/外链占位卡片。
    fn render_image_block(
        &mut self,
        rt: &ID2D1RenderTarget,
        brushes: &FrameBrushes,
        metric: &BlockMetric,
        url: &str,
        alt: &str,
    ) {
        self.ensure_image_loaded(url);
        let top = metric.y_top - self.scroll_y;
        let x = metric.text_x;
        let w = metric.text_w;

        match self.image_cache.get(url) {
            Some(CachedImage::Loaded {
                width,
                height,
                pixels,
                ..
            }) if *width > 0 && *height > 0 => {
                let bmp_props = D2D1_BITMAP_PROPERTIES {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                };
                let bitmap_res: windows::core::Result<ID2D1Bitmap> = unsafe {
                    rt.CreateBitmap(
                        D2D_SIZE_U {
                            width: *width,
                            height: *height,
                        },
                        Some(pixels.as_ptr().cast::<c_void>()),
                        *width * 4,
                        &bmp_props,
                    )
                };
                if let Ok(bitmap) = bitmap_res {
                    let scale = (w / *width as f32).min(1.0);
                    let disp_w = (*width as f32 * scale).max(16.0);
                    let disp_h = (*height as f32 * scale).clamp(24.0, 640.0);
                    let img_x = x + (w - disp_w) * 0.5;
                    let dest = D2D_RECT_F {
                        left: img_x,
                        top,
                        right: img_x + disp_w,
                        bottom: top + disp_h,
                    };
                    unsafe {
                        rt.DrawBitmap(
                            &bitmap,
                            Some(&dest),
                            1.0,
                            windows::Win32::Graphics::Direct2D::D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                            None,
                        );
                        rt.DrawRectangle(&dest, &brushes.border, 1.0, None);
                    }
                    if !alt.trim().is_empty()
                        && let Ok(cfmt) = self.create_text_format(
                            self.body_font_name(),
                            12.5,
                            DWRITE_FONT_WEIGHT_NORMAL,
                            true,
                            1.3,
                            DWRITE_TEXT_ALIGNMENT_CENTER,
                        )
                    {
                        let cw: Vec<u16> = alt.encode_utf16().collect();
                        if let Ok(clayout) =
                            unsafe { self.dwrite_factory.CreateTextLayout(&cw, &cfmt, w, 24.0) }
                        {
                            unsafe {
                                rt.DrawTextLayout(
                                    Vector2 {
                                        X: x,
                                        Y: top + disp_h + 4.0,
                                    },
                                    &clayout,
                                    &brushes.muted,
                                    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                                );
                            }
                        }
                    }
                    return;
                }
            }
            _ => {}
        }

        // 占位卡片（损坏图片或缺失图片零崩溃兜底）
        let reason = match self.image_cache.get(url) {
            Some(CachedImage::Failed { reason, .. }) => reason.clone(),
            _ => format!("无法显示图片: {url}"),
        };
        let label = if alt.trim().is_empty() {
            format!("[图片占位] {reason}")
        } else {
            format!("[图片: {alt}] {reason}")
        };
        let rr = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: x,
                top,
                right: x + w,
                bottom: top + 52.0,
            },
            radiusX: 6.0,
            radiusY: 6.0,
        };
        unsafe {
            rt.FillRoundedRectangle(&rr, &brushes.code_bg);
            rt.DrawRoundedRectangle(&rr, &brushes.border, 1.0, None);
        }
        if let Ok(fmt) = self.create_text_format(
            self.body_font_name(),
            13.0,
            DWRITE_FONT_WEIGHT_NORMAL,
            false,
            1.4,
            DWRITE_TEXT_ALIGNMENT_LEADING,
        ) {
            let lw: Vec<u16> = label.encode_utf16().collect();
            if let Ok(layout) =
                unsafe { self.dwrite_factory.CreateTextLayout(&lw, &fmt, (w - 28.0).max(80.0), 40.0) }
            {
                unsafe {
                    rt.DrawTextLayout(
                        Vector2 {
                            X: x + 14.0,
                            Y: top + 16.0,
                        },
                        &layout,
                        &brushes.muted,
                        D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                    );
                }
            }
        }
    }

    /// 命中测试文档坐标 `(x_dip, doc_y_dip)`，返回逻辑位置 `(DocPos, Option<link_url>)`。
    fn hit_test_doc(&mut self, x_dip: f32, doc_y: f32) -> (Option<DocPos>, Option<String>) {
        self.rebuild_layout_metrics_if_needed();
        if self.metrics.is_empty() {
            return (None, None);
        }

        // 查找最接近的文本块
        let mut target_idx = None;
        for (i, m) in self.metrics.iter().enumerate() {
            if matches!(
                self.doc.blocks[i].kind,
                BlockKind::ThematicBreak | BlockKind::Image { .. } | BlockKind::Table { .. }
            ) {
                continue;
            }
            if doc_y >= m.y_top && doc_y <= m.y_top + m.height {
                target_idx = Some(i);
                break;
            }
            if doc_y < m.y_top {
                target_idx = Some(i);
                break;
            }
            target_idx = Some(i);
        }

        let Some(idx) = target_idx else {
            return (None, None);
        };
        let metric = self.metrics[idx].clone();
        let Some(entry) = self.get_or_build_block_layout(idx) else {
            return (None, None);
        };
        let layout = entry.layout.clone();
        let ranges = entry.ranges.clone();

        let local_x = (x_dip - metric.text_x).max(0.0);
        let local_y = (doc_y - metric.text_y).max(0.0);

        let mut is_trailing = windows::core::BOOL(0);
        let mut is_inside = windows::core::BOOL(0);
        let mut htm = DWRITE_HIT_TEST_METRICS::default();
        if unsafe {
            layout.HitTestPoint(local_x, local_y, &mut is_trailing, &mut is_inside, &mut htm)
        }
        .is_err()
        {
            return (
                Some(DocPos {
                    block: idx,
                    utf16_offset: 0,
                }),
                None,
            );
        }

        let mut offset = htm.textPosition;
        if is_trailing.as_bool() {
            offset = offset.saturating_add(htm.length);
        }
        offset = offset.min(metric.utf16_len);

        let link = if is_inside.as_bool()
            && x_dip >= metric.text_x
            && x_dip <= metric.text_x + metric.text_w
            && doc_y >= metric.text_y
            && doc_y <= metric.y_top + metric.height
        {
            ranges
                .iter()
                .find(|r| htm.textPosition >= r.start && htm.textPosition < r.start + r.len)
                .and_then(|r| r.link_url.clone())
        } else {
            None
        };

        (
            Some(DocPos {
                block: idx,
                utf16_offset: offset,
            }),
            link,
        )
    }

    fn normalized_selection(&self) -> (Option<DocPos>, Option<DocPos>) {
        match (self.sel_anchor, self.sel_focus) {
            (Some(a), Some(b)) if a != b => {
                if a <= b {
                    (Some(a), Some(b))
                } else {
                    (Some(b), Some(a))
                }
            }
            _ => (None, None),
        }
    }

    /// 提取当前跨块选区的精确 UTF-8 文本。
    fn extract_selected_text(&mut self) -> String {
        let (Some(a), Some(b)) = self.normalized_selection() else {
            return String::new();
        };
        let mut out_parts = Vec::new();
        for idx in a.block..=b.block.min(self.doc.blocks.len().saturating_sub(1)) {
            if matches!(
                self.doc.blocks[idx].kind,
                BlockKind::ThematicBreak | BlockKind::Image { .. } | BlockKind::Table { .. }
            ) {
                continue;
            }
            let is_code = matches!(self.doc.blocks[idx].kind, BlockKind::CodeBlock { .. });
            let flat = flatten_spans(&self.doc.blocks[idx].spans, is_code);
            let wide: Vec<u16> = flat.text.encode_utf16().collect();
            let s0 = if idx == a.block {
                (a.utf16_offset as usize).min(wide.len())
            } else {
                0
            };
            let s1 = if idx == b.block {
                (b.utf16_offset as usize).min(wide.len())
            } else {
                wide.len()
            };
            if s1 > s0 {
                // 剔除行内代码胶囊左右辅助排版薄空格，保证剪贴板文本纯净无损
                let cleaned = String::from_utf16_lossy(&wide[s0..s1]).replace('\u{2009}', "");
                out_parts.push(cleaned);
            }
        }
        out_parts.join("\n")
    }

    fn on_mouse_down(&mut self, x_px: i32, y_px: i32) {
        let _ = unsafe { SetFocus(Some(self.hwnd)) };
        let x = self.px_to_dip(x_px);
        let y = self.px_to_dip(y_px);
        let doc_y = y + self.scroll_y;
        let (pos, _) = self.hit_test_doc(x, doc_y);

        self.selecting = true;
        self.drag_start_dip = (x, y);
        self.drag_moved = false;
        self.sel_anchor = pos;
        self.sel_focus = pos;
        unsafe {
            SetCapture(self.hwnd);
        }
        self.invalidate();
    }

    fn on_mouse_move(&mut self, x_px: i32, y_px: i32) {
        let x = self.px_to_dip(x_px);
        let y = self.px_to_dip(y_px);
        let doc_y = y + self.scroll_y;

        if self.selecting {
            let dist = (x - self.drag_start_dip.0).abs() + (y - self.drag_start_dip.1).abs();
            if dist > 4.0 {
                self.drag_moved = true;
            }
            let (pos, _) = self.hit_test_doc(x, doc_y);
            if pos.is_some() {
                self.sel_focus = pos;
                // 若起始点落在版心外侧空白且拖动距离明显但首尾恰好重叠，自动扩展选中覆盖块内文本
                if self.drag_moved
                    && let (Some(a), Some(b)) = (self.sel_anchor, self.sel_focus)
                    && a == b
                    && let Some(m) = self.metrics.get(a.block)
                    && m.utf16_len > 0
                {
                    self.sel_anchor = Some(DocPos {
                        block: a.block,
                        utf16_offset: 0,
                    });
                    self.sel_focus = Some(DocPos {
                        block: a.block,
                        utf16_offset: m.utf16_len,
                    });
                }
                self.invalidate();
            }
            return;
        }

        let (_, link) = self.hit_test_doc(x, doc_y);
        self.hovered_link = link;
    }

    fn on_mouse_up(&mut self, x_px: i32, y_px: i32) {
        if !self.selecting {
            return;
        }
        self.selecting = false;
        let _ = unsafe { ReleaseCapture() };

        let x = self.px_to_dip(x_px);
        let y = self.px_to_dip(y_px);
        let doc_y = y + self.scroll_y;
        let dist = (x - self.drag_start_dip.0).abs() + (y - self.drag_start_dip.1).abs();
        if dist > 4.0 {
            self.drag_moved = true;
        }

        if self.drag_moved {
            let (pos, _) = self.hit_test_doc(x, doc_y);
            if pos.is_some() {
                self.sel_focus = pos;
            }
            // 兜底：若拖拽跨度 > 12 DIP 但起点/终点都在同一行右侧或左侧空白，回退到首个可见非空块选区
            if self.normalized_selection().0.is_none() && dist > 12.0 {
                let top_y = self.drag_start_dip.1.min(y) + self.scroll_y;
                let bot_y = self.drag_start_dip.1.max(y) + self.scroll_y;
                if let Some((bi, m)) = self
                    .metrics
                    .iter()
                    .enumerate()
                    .find(|(_, m)| m.utf16_len > 0 && m.y_top + m.height >= top_y && m.y_top <= bot_y + 40.0)
                {
                    self.sel_anchor = Some(DocPos {
                        block: bi,
                        utf16_offset: 0,
                    });
                    self.sel_focus = Some(DocPos {
                        block: bi,
                        utf16_offset: m.utf16_len.min(120),
                    });
                }
            }
        } else {
            // 单击：清空选区；若点击安全超链接则打开外部链接
            self.sel_anchor = None;
            self.sel_focus = None;
            let (_, link) = self.hit_test_doc(x, doc_y);
            if let Some(url) = link {
                open_safe_link(&url);
            }
        }
        self.invalidate();
    }

    /// 更新页内查找关键词（大小写不敏感匹配所有块文本与表格单元格）。
    fn update_find_query(&mut self, query: &str, auto_scroll: bool) {
        self.rebuild_layout_metrics_if_needed();
        self.find_query = query.to_string();
        self.find_matches.clear();
        self.active_match = None;

        if query.is_empty() {
            self.invalidate();
            return;
        }

        let needle_lower = query.to_lowercase();
        let needle_utf16_len = query.encode_utf16().count() as u32;
        if needle_utf16_len == 0 {
            self.invalidate();
            return;
        }

        for (b_idx, block) in self.doc.blocks.iter().enumerate() {
            let base_y = self.metrics.get(b_idx).map_or(0.0, |m| m.y_top);
            match &block.kind {
                BlockKind::Table { rows, .. } => {
                    let row_heights = self
                        .table_metrics
                        .get(&b_idx)
                        .map(|t| t.row_heights.as_slice())
                        .unwrap_or(&[]);
                    let mut ry = base_y;
                    for (r_idx, row) in rows.iter().enumerate() {
                        for (c_idx, cell) in row.iter().enumerate() {
                            let flat = flatten_spans(cell, false);
                            for utf16_start in find_utf16_matches_ci(&flat.text, &needle_lower) {
                                self.find_matches.push(FindMatch {
                                    block: b_idx,
                                    cell_idx: Some((r_idx, c_idx)),
                                    utf16_start,
                                    utf16_len: needle_utf16_len,
                                    approx_y: ry,
                                });
                            }
                        }
                        ry += row_heights.get(r_idx).copied().unwrap_or(32.0);
                    }
                }
                BlockKind::ThematicBreak | BlockKind::Image { .. } => {}
                _ => {
                    let is_code = matches!(block.kind, BlockKind::CodeBlock { .. });
                    let flat = flatten_spans(&block.spans, is_code);
                    for utf16_start in find_utf16_matches_ci(&flat.text, &needle_lower) {
                        self.find_matches.push(FindMatch {
                            block: b_idx,
                            cell_idx: None,
                            utf16_start,
                            utf16_len: needle_utf16_len,
                            approx_y: base_y,
                        });
                    }
                }
            }
        }

        if !self.find_matches.is_empty() {
            self.active_match = Some(0);
            if auto_scroll {
                self.scroll_to_active_match();
            }
        }
        self.invalidate();
    }

    fn step_find_match(&mut self, forward: bool) {
        let len = self.find_matches.len();
        if len == 0 {
            return;
        }
        let cur = self.active_match.unwrap_or(0);
        let next = if forward {
            (cur + 1) % len
        } else {
            (cur + len - 1) % len
        };
        self.active_match = Some(next);
        self.scroll_to_active_match();
        self.invalidate();
    }

    fn scroll_to_active_match(&mut self) {
        let Some(idx) = self.active_match else {
            return;
        };
        let Some(m) = self.find_matches.get(idx) else {
            return;
        };
        let target_y = (m.approx_y - self.client_h * 0.28).max(0.0);
        self.scroll_y = target_y;
        self.clamp_scroll();
    }

    fn find_status_text(&self) -> String {
        if self.find_query.is_empty() {
            return String::new();
        }
        if self.find_matches.is_empty() {
            return "0 命中".to_string();
        }
        let cur = self.active_match.unwrap_or(0) + 1;
        format!("{cur}/{}", self.find_matches.len())
    }
}

/// 行内代码胶囊外侧极窄排版间隔符（防止胶囊圆角边框与相邻中文标点或汉字碰撞穿模）。
const INLINE_CODE_OUTER_PAD: &str = "\u{2009}";

/// 将 `Vec<RichSpan>` 展平为连续字符串并记录各片段 UTF-16 范围。
///
/// 非围栏代码块中的行内代码片段两侧自动追加 `\u{2009}` 细间距，并在内侧保留微距，
/// 确保胶囊边框与相邻中文句号、括号和正文完全零重叠。
fn flatten_spans(spans: &[RichSpan], is_code_block: bool) -> FlattenedSpans {
    let mut text = String::new();
    let mut ranges = Vec::with_capacity(spans.len());
    let mut offset = 0u32;

    for (i, sp) in spans.iter().enumerate() {
        if sp.text.is_empty() {
            continue;
        }
        let need_capsule_pad = !is_code_block && sp.code;
        if need_capsule_pad && i > 0 {
            text.push_str(INLINE_CODE_OUTER_PAD);
            offset = offset.saturating_add(1);
        }
        let start = offset;
        let mut inner_len = 0u32;
        if need_capsule_pad {
            text.push_str(INLINE_CODE_OUTER_PAD);
            inner_len += 1;
        }
        text.push_str(&sp.text);
        inner_len += sp.text.encode_utf16().count() as u32;
        if need_capsule_pad {
            text.push_str(INLINE_CODE_OUTER_PAD);
            inner_len += 1;
        }
        ranges.push(SpanRange {
            start,
            len: inner_len,
            bold: sp.bold,
            italic: sp.italic,
            strike: sp.strike,
            code: sp.code,
            link_url: sp.link_url.clone(),
            highlight: sp.highlight,
        });
        offset = offset.saturating_add(inner_len);
        if need_capsule_pad && i + 1 < spans.len() {
            text.push_str(INLINE_CODE_OUTER_PAD);
            offset = offset.saturating_add(1);
        }
    }

    FlattenedSpans {
        text,
        utf16_len: offset,
        ranges,
    }
}

/// 大小写不敏感查找文本中所有命中位置的 UTF-16 偏移（支持中文、Emoji 与多字节 UTF-8）。
fn find_utf16_matches_ci(haystack: &str, needle_lower: &str) -> Vec<u32> {
    if haystack.is_empty() || needle_lower.is_empty() {
        return Vec::new();
    }
    let hay_chars: Vec<char> = haystack.chars().collect();
    let hay_lower: Vec<char> = hay_chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();
    let needle_chars: Vec<char> = needle_lower.chars().collect();
    let nlen = needle_chars.len();
    if nlen == 0 || hay_lower.len() < nlen {
        return Vec::new();
    }

    // 预构建字符索引 -> UTF-16 偏移映射，保证 DirectWrite 偏移 100% 精确
    let mut utf16_prefix = Vec::with_capacity(hay_chars.len() + 1);
    let mut acc = 0u32;
    utf16_prefix.push(0);
    for ch in &hay_chars {
        acc += ch.len_utf16() as u32;
        utf16_prefix.push(acc);
    }

    let mut hits = Vec::new();
    let mut i = 0usize;
    while i + nlen <= hay_lower.len() {
        if hay_lower[i..i + nlen] == needle_chars[..] {
            hits.push(utf16_prefix[i]);
            i += nlen;
        } else {
            i += 1;
        }
    }
    hits
}

/// 使用 `IDWriteTextLayout::HitTestTextRange` 绘制一段文本的多行高亮/胶囊矩形。
#[allow(clippy::too_many_arguments)]
fn draw_text_range_rects(
    rt: &ID2D1RenderTarget,
    layout: &IDWriteTextLayout,
    origin_x: f32,
    origin_y: f32,
    start: u32,
    len: u32,
    fill: &ID2D1SolidColorBrush,
    stroke: Option<&ID2D1SolidColorBrush>,
    pad_x: f32,
    radius: f32,
) {
    if len == 0 {
        return;
    }
    let mut actual_count = 0u32;
    let _ = unsafe { layout.HitTestTextRange(start, len, origin_x, origin_y, None, &mut actual_count) };
    if actual_count == 0 {
        return;
    }
    let mut buf = vec![DWRITE_HIT_TEST_METRICS::default(); actual_count as usize];
    if unsafe {
        layout.HitTestTextRange(
            start,
            len,
            origin_x,
            origin_y,
            Some(&mut buf),
            &mut actual_count,
        )
    }
    .is_err()
    {
        return;
    }

    for m in buf.iter().take(actual_count as usize) {
        if m.width <= 0.0 || m.height <= 0.0 {
            continue;
        }
        let rect = D2D_RECT_F {
            left: m.left - pad_x,
            top: m.top + 1.0,
            right: m.left + m.width + pad_x,
            bottom: m.top + m.height - 1.0,
        };
        if radius > 0.0 {
            let rr = D2D1_ROUNDED_RECT {
                rect,
                radiusX: radius,
                radiusY: radius,
            };
            unsafe {
                rt.FillRoundedRectangle(&rr, fill);
                if let Some(sb) = stroke {
                    rt.DrawRoundedRectangle(&rr, sb, 0.8, None);
                }
            }
        } else {
            unsafe {
                rt.FillRectangle(&rect, fill);
            }
        }
    }
}

fn table_align_to_dwrite(
    align: TableAlign,
) -> windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT {
    match align {
        TableAlign::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
        TableAlign::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
        TableAlign::Left | TableAlign::None => DWRITE_TEXT_ALIGNMENT_LEADING,
    }
}

/// 解析相对或绝对本地图片路径（支持 URL 空格 `%20` 简单解码）。
fn resolve_local_image_path(url: &str, doc_dir: Option<&Path>) -> Option<PathBuf> {
    let clean = url.trim().trim_matches('<').trim_matches('>');
    if clean.is_empty() {
        return None;
    }
    let decoded = clean.replace("%20", " ");
    let p = Path::new(&decoded);
    if p.is_absolute() {
        Some(p.to_path_buf())
    } else {
        doc_dir.map(|dir| dir.join(p))
    }
}

/// 仅允许安全外部协议（`http://`、`https://`、`mailto:`）按需动态调用 `shell32.dll!ShellExecuteW` 打开。
fn open_safe_link(url: &str) {
    type ShellExecuteWFn = unsafe extern "system" fn(
        HWND,
        PCWSTR,
        PCWSTR,
        PCWSTR,
        PCWSTR,
        i32,
    ) -> HINSTANCE;
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:"))
    {
        return;
    }
    let wide: Vec<u16> = trimmed.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if let Ok(hmod) = LoadLibraryW(w!("shell32.dll"))
            && let Some(proc) = GetProcAddress(hmod, windows::core::s!("ShellExecuteW"))
        {
            let func: ShellExecuteWFn = std::mem::transmute(proc);
            let _ = func(
                HWND::default(),
                w!("open"),
                PCWSTR(wide.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL.0,
            );
        }
    }
}

/// 使用 Win32 Clipboard API 将文本写入 `CF_UNICODETEXT`。
pub fn write_clipboard_unicode(text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let byte_len = wide.len() * std::mem::size_of::<u16>();

    for _ in 0..6 {
        if unsafe { OpenClipboard(None) }.is_ok() {
            let _ = unsafe { EmptyClipboard() };
            let Ok(hmem): windows::core::Result<HGLOBAL> = (unsafe { GlobalAlloc(GHND, byte_len) })
            else {
                let _ = unsafe { CloseClipboard() };
                return false;
            };
            let ptr = unsafe { GlobalLock(hmem) };
            if ptr.is_null() {
                let _ = unsafe { GlobalFree(Some(hmem)) };
                let _ = unsafe { CloseClipboard() };
                return false;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr.cast::<u16>(), wide.len());
                let _ = GlobalUnlock(hmem);
            }
            let ok = unsafe {
                SetClipboardData(
                    CF_UNICODETEXT.0 as u32,
                    Some(windows::Win32::Foundation::HANDLE(hmem.0)),
                )
            }
            .is_ok();
            if !ok {
                let _ = unsafe { GlobalFree(Some(hmem)) };
            }
            let _ = unsafe { CloseClipboard() };
            return ok;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

fn register_reader_window_class(instance: HINSTANCE) -> windows::core::Result<()> {
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
        lpfnWndProc: Some(reader_wnd_proc),
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        lpszClassName: READER_CLASS_NAME,
        ..Default::default()
    };
    let atom = unsafe { RegisterClassW(&wc) };
    if atom == 0 {
        let err = windows::core::Error::from_thread();
        // ERROR_CLASS_ALREADY_EXISTS (1410) 可安全忽略
        if err.code() != windows::core::HRESULT::from_win32(1410)
            && err.code() != windows::core::HRESULT::from_win32(ERROR_SUCCESS.0)
        {
            return Err(err);
        }
    }
    Ok(())
}

/// 从 `LPARAM` 提取带符号 16 位鼠标坐标（兼容负坐标与多显示器）。
fn lparam_xy(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    (x, y)
}

unsafe extern "system" fn reader_wnd_proc(
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

    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ReaderViewState };
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    let state = unsafe { &mut *ptr };

    match msg {
        WM_GETOBJECT => {
            if let Some(res) = uia::handle_wm_getobject(hwnd, wparam, lparam, "IDC_READER") {
                return res;
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_SIZE => {
            let w_px = (lparam.0 & 0xFFFF) as u16 as i32;
            let h_px = ((lparam.0 >> 16) & 0xFFFF) as u16 as i32;
            state.refresh_dpi();
            let new_w = state.px_to_dip(w_px.max(1));
            let new_h = state.px_to_dip(h_px.max(1));
            if (new_w - state.client_w).abs() > 0.5 {
                state.layout_dirty = true;
            }
            state.client_w = new_w;
            state.client_h = new_h;
            state.rebuild_layout_metrics_if_needed();
            state.clamp_scroll();
            state.invalidate();
            LRESULT(0)
        }
        WM_PAINT => {
            state.on_paint();
            LRESULT(0)
        }
        WM_PRINT | WM_PRINTCLIENT => {
            state.on_print_client(HDC(wparam.0 as *mut c_void));
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as i16;
            let lines = -(f32::from(delta) / 120.0);
            let step = (state.config.font_size * state.config.line_height * 3.0).max(56.0);
            state.rebuild_layout_metrics_if_needed();
            state.scroll_y += lines * step;
            state.clamp_scroll();
            state.invalidate();
            if let Ok(parent) = unsafe { windows::Win32::UI::WindowsAndMessaging::GetParent(hwnd) }
                && !parent.is_invalid()
                && !parent.0.is_null()
            {
                let _ = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                        Some(parent),
                        crate::ui::WM_APP_READER_SCROLLED,
                        WPARAM(0),
                        LPARAM(0),
                    )
                };
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = lparam_xy(lparam);
            state.on_mouse_down(x, y);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = lparam_xy(lparam);
            state.on_mouse_move(x, y);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = lparam_xy(lparam);
            state.on_mouse_up(x, y);
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let low = (lparam.0 & 0xFFFF) as u16;
            if low == 1 {
                // HTCLIENT
                let cur_id = if state.hovered_link.is_some() {
                    IDC_HAND
                } else {
                    IDC_IBEAM
                };
                if let Ok(cur) = unsafe { LoadCursorW(None, cur_id) } {
                    unsafe { SetCursor(Some(cur)) };
                    return LRESULT(1);
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_NCDESTROY => {
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let _ = Box::from_raw(ptr);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// 抑制未使用导入警告（占位供跨模块坐标/色值扩展使用）。
#[allow(dead_code)]
fn _keep_types_referenced(_: POINT, _: COLORREF) {}

#[cfg(test)]
mod tests {
    use super::{find_utf16_matches_ci, flatten_spans, resolve_local_image_path};
    use crate::core::markdown::RichSpan;
    use std::path::Path;

    #[test]
    fn test_find_utf16_matches_ci_ascii_and_cjk_surrogate() {
        let text = "Hello 😀 世界 NeedleKeyword and needlekeyword!";
        let hits = find_utf16_matches_ci(text, "needlekeyword");
        assert_eq!(hits.len(), 2);
        let wide: Vec<u16> = text.encode_utf16().collect();
        let m1 = String::from_utf16_lossy(&wide[hits[0] as usize..hits[0] as usize + 13]);
        let m2 = String::from_utf16_lossy(&wide[hits[1] as usize..hits[1] as usize + 13]);
        assert_eq!(m1, "NeedleKeyword");
        assert_eq!(m2, "needlekeyword");
    }

    #[test]
    fn test_flatten_spans_preserves_utf16_offsets() {
        let spans = vec![
            RichSpan {
                text: "中文".to_string(),
                bold: true,
                italic: false,
                strike: false,
                code: false,
                link_url: None,
                highlight: None,
            },
            RichSpan {
                text: "code".to_string(),
                bold: false,
                italic: false,
                strike: false,
                code: true,
                link_url: None,
                highlight: None,
            },
        ];
        let flat = flatten_spans(&spans, true);
        assert_eq!(flat.text, "中文code");
        assert_eq!(flat.utf16_len, 6);
        assert_eq!(flat.ranges[0].start, 0);
        assert_eq!(flat.ranges[0].len, 2);
        assert_eq!(flat.ranges[1].start, 2);
        assert_eq!(flat.ranges[1].len, 4);

        // 非代码块内行内代码胶囊左右自动附带细空格隔离，避免压穿相邻标点
        let inline_flat = flatten_spans(&spans, false);
        assert!(inline_flat.text.contains('\u{2009}'));
        assert_eq!(inline_flat.text.replace('\u{2009}', ""), "中文code");
    }

    #[test]
    fn test_resolve_local_image_path() {
        let base = Path::new(r"C:\Docs\notes");
        let p = resolve_local_image_path("images/demo%20pic.png", Some(base)).unwrap();
        assert_eq!(p, Path::new(r"C:\Docs\notes\images/demo pic.png"));
    }
}
