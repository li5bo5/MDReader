# MDReader 实施方案与架构设计文档

> **文档状态**：Milestone 1（`docs-and-scaffold`）定稿蓝图，M2–M6 开发与验收唯一基准  
> **适用平台**：Windows 10 / Windows 11（x64 原生桌面端）  
> **编写原则**：严格遵循 **`1. 内存 > 2. UI 美化 > 3. 数据安全`** 优先级铁律  
> **编写语言**：简体中文

---

## 1. 项目定位、需求边界与优先级准则

### 1.1 核心定位

**MDReader** 是一款面向个人日常阅读与轻量修改场景的纯粹 Windows 桌面端 Markdown 阅读器。软件聚焦“双击即读、舒适排版、一键修订、保存即归位”的核心闭环：

1. **Windows `.md` 原生打开方式**：支持无管理员权限注册为 `.md` / `.markdown` 打开方式候选应用，支持资源管理器双击、右键“打开方式”、命令行传参以及窗口拖放直接打开文档。
2. **纯粹低干扰阅读体验**：对标 `mdfiler.com` 的去工作台化居中限宽版心与高对比度柔和配色，完整清晰渲染常用 GFM 元素。
3. **阅读 ↔ 修改一键闭环**：默认直接进入阅读模式；通过顶部单一极简按钮或快捷键（`Ctrl+E` / `F2`）切入轻量纯文本修改模式；按 `Ctrl+S` 或点击保存后，原子写回原磁盘文件并**立即自动切回阅读模式**展示最新渲染结果。
4. **极简界面与单一本地 JSON 配置**：借鉴 `smartpdf`（SumatraPDF）设置哲学，界面中不堆砌任何设置面板、选项卡或弹窗，所有外观与行为偏好集中收敛于一份本地 `config.json`。

### 1.2 优先级铁律及其刚性架构约束

| 优先级 | 维度 | 刚性设计约束 |
|---|---|---|
| **P1（最高）** | **内存占用** | 严禁引入 Electron / WebView2 / Chromium 多进程沙箱，严禁引入 CoreCLR / JVM 运行时。采用单进程原生 Win32 + Direct2D/DirectWrite + `Msftedit.dll`，共享系统字体映射并实施按需视口裁剪与 LRU 布局缓存，保证冷启动与典型文档进程 `WorkingSet64` 稳定控制在 20–30 MB 以内（预期 8–22 MB）。 |
| **P2（次高）** | **UI 美化与阅读体验** | 不因原生轻量化牺牲排版美感。采用 DirectWrite ClearType 亚像素抗锯齿渲染文本、Direct2D 绘制高 DPI 矢量圆角代码块/引用竖线/任务复选框/表格网格、WIC 解码本地图片；默认 760px 居中限宽、1.65 倍行高与 WCAG AAA 明暗低眩光配色。 |
| **P3（基础保障）** | **数据安全** | 所有修改写回必须采用“同卷临时文件 + `FlushFileBuffers` + `ReplaceFileW`（回退 `MoveFileExW`）”原子替换流程；保留原文件 UTF-8 BOM 与 LF/CRLF 行尾；写回失败绝不切出编辑区；配置文件解析缺失或损坏时逐字段/整体安全回退默认值，绝不破坏用户配置文件。 |

### 1.3 非目标（Non-Goals / 明确剔除边界）

为守住内存底线与极简阅读心智，本项目**明确不做**以下特性：

- 不做多标签页浏览器、文件树侧栏、大纲导航侧栏或收藏/标签数据库；
- 不做所见即所得（WYSIWYG）混合富文本编辑器、打字机模式或分屏实时预览；
- 不做图形化偏好设置弹窗、向导页或内置多语言设置中心；
- 不做账号登录、云端同步、插件市场或内置 Git 工作流；
- 不做内嵌浏览器引擎、Mermaid/LaTeX/PDF 重型运行时渲染与多格式导出器。

---

## 2. 技术选型论证与候选栈全景对比

### 2.1 本机实测工具链基线

依据项目初始化阶段对目标构建主机（Windows 11 专业版 `10.0.26200` x64）的实测探测结果：

- **Rust 工具链就绪**：`rustc 1.99.1`、`cargo 1.99.1`、`clippy 0.1.99`、`rustfmt`（均为 `stable-x86_64-pc-windows-msvc`），无需手动加载 `vcvars64.bat` 即可自动链接本机 **MSVC `14.44.35207`** 与 **Windows 11 SDK `10.0.26100.0`**。
- **全局镜像就绪**：本机 `%USERPROFILE%\.cargo\config.toml` 已全局配置清华 `crates.io` 稀疏索引（`sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/`），因此**项目仓库内不创建 `.cargo/config.toml`**，避免把个人镜像路径固化到代码库。
- **全局缺失构建工具**：`cmake`、`ninja`、`clang`、`g++`、`go` 均未在全局 `PATH` 安装。
- **重型运行时基线**：系统虽预装 WebView2 Runtime `157.0.4341.60` 与 .NET SDK `8.0.422`，但受多进程渲染器或 CLR 内存基线限制，不符合第一优先级。

### 2.2 六大候选技术路线对比矩阵

| 技术路线 | 进程模型 | 预期 Working Set（空载 / 典型） | UI 美化与 GFM 上限 | 中文 IME 与划词 | 构建工具链依赖 | UIA 可测性与综合结论 |
|---|---|---|---|---|---|---|
| **1. Electron** | 多进程 Chromium + Node.js | 150–300+ MB | 极高（HTML/CSS） | 浏览器级完整支持 | Node/npm + 打包器 | 支持，但内存严重超标，**淘汰** |
| **2. WebView2 (Tauri v2 / Wails / C++)** | 宿主 + 4~5 个 `msedgewebview2.exe` | 80–250+ MB（物理硬地板 >40–90 MB） | 极高（HTML/CSS） | 浏览器级完整支持 | Rust/Go + 前端资源 | 进程树出现 WebView2，违背内存第一原则，**淘汰** |
| **3. .NET 8 WPF / WinUI 3** | 单进程 + CoreCLR JIT | 45–160 MB（不支持原生零运行时） | 高（XAML 排版） | 原生支持 | .NET 8 SDK | 空载内存明显高于纯原生，**淘汰** |
| **4. Rust 自绘 GUI (`egui` / `iced`)** | 单进程 + GPU/软件光栅图集 | 35–55 MB（CJK 字形图集占内存） | 中（默认缺 ClearType 锐度） | 跨块自由划词与中文 IME 弱 | 纯 Cargo | 缺少原生 UIA 文本树，自动化与文本锐度差，**淘汰** |
| **5. C++20 + Win32 + D2D/DWrite + MD4C** | 单进程纯原生 Win32/COM | 8–22 MB | 极高（ClearType + D2D 矢量） | 原生支持（DWrite + `RICHEDIT50W`） | 缺全局 CMake/Ninja | 运行期能力顶尖，但无全局构建编排，维护成本高，**次选** |
| **6. Rust (MSVC) + Win32 (`windows` 0.62) + Direct2D/DirectWrite/WIC + `pulldown-cmark` + `Msftedit.dll` + `serde_json`** | **单进程纯原生 Win32/COM** | **8–22 MB（门槛 <20 / <30 MB）** | **极高（ClearType + 矢量绘制 + WIC 图片）** | **原生 TSF IME + DWrite 命中划词** | **纯 Cargo，零 CMake/C 编译依赖** | **原生 Win32 `BUTTON` + `RICHEDIT50W` 原生 UIA；Cargo 单测完备，内存安全与 COM RAII 兼得，最终选定** |

### 2.3 核心 Crate 选型、特性裁剪与实测避坑规范

通过独立临时工程在 `rustc 1.99.1` / `edition = "2024"` 下全量编译与 `cargo clippy --all-targets -- -D warnings` 实测，确定如下 `Cargo.toml` 策略：

1. **`pulldown-cmark = { version = "0.13", default-features = false }`（实测解析 `0.13.6`）**
   - **关键裁剪**：必须禁用默认 features（默认包含 `["getopts", "html"]`，会额外拉入 CLI 解析库 `getopts`、`unicode-width` 与 `pulldown-cmark-escape`）。MDReader 直接将事件流转换为 Direct2D/DirectWrite 排版模型而不输出 HTML，禁用默认特性后依然完整保留 GFM 扩展（`Options::ENABLE_TABLES`、`ENABLE_STRIKETHROUGH`、`ENABLE_TASKLISTS`），仅传递依赖 `bitflags`、`memchr`、`unicase`。
   - **API 兼容提示**：`0.13.x` 中 `Tag::Heading { level, id, classes, attrs }`、`Tag::Link`/`Tag::Image` 为结构体变体，`Tag::BlockQuote(Option<BlockQuoteKind>)` 携带引用类型。

2. **`serde = { version = "1.0", features = ["derive"] }` + `serde_json = "1.0"`（实测 `1.0.229` / `1.0.154`）**
   - 用于 `AppConfig` 结构定义与先解析为 `serde_json::Value` 再逐字段容错回退的加载逻辑。

3. **`windows = "0.62"`（实测 `0.62.2`）+ `windows-numerics = "0.3"`（实测 `0.3.1`）**
   - `windows 0.62.2` 采用轻量化 `windows-link` 直连系统 DLL，零构建脚本与 C/C++ 编译耗时。
   - **必须精确开启的 26 项 Win32 Features 与实测避坑点**：
     - 基础与存储：`Win32_Foundation`、`Win32_Storage_FileSystem`、`Win32_System_Threading`、`Win32_System_LibraryLoader`
     - 注册表与安全门控：`Win32_System_Registry` + **`Win32_Security`**（在 `windows 0.62.2` 中 `RegCreateKeyExW` 受 `Win32_Security` 门控，缺失将编译失败）
     - Direct2D / DirectWrite / WIC：`Win32_Graphics_Direct2D`、`Win32_Graphics_Direct2D_Common`、`Win32_Graphics_Dxgi_Common`、`Win32_Graphics_DirectWrite`、`Win32_Graphics_Imaging`、`Win32_Graphics_Imaging_D2D`（注：`CreateBitmapFromWicBitmap` 受 `Direct2D_Common` + `Dxgi_Common` + `Imaging` 三者联合门控；`D2D_POINT_2F` 已由 `windows_numerics::Vector2` 替代，故显式引入 `windows-numerics = "0.3"`）
     - GDI / DWM / 窗口 / DPI：`Win32_Graphics_Gdi`、`Win32_Graphics_Dwm`、`Win32_UI_WindowsAndMessaging`、`Win32_UI_HiDpi`、`Win32_UI_Input_KeyboardAndMouse`（注：`BOOL` 位于 `windows::core::BOOL`；`SetFocus` 位于 `Input::KeyboardAndMouse`；`GetScrollInfo` 位于 `WindowsAndMessaging` 而 `SetScrollInfo` 位于 `UI::Controls`）
     - 控件与编辑器：`Win32_UI_Controls`、`Win32_UI_Controls_RichEdit`（`EM_SETRECT`/`EM_GETMODIFY`/`EM_SETMODIFY`/`EM_EMPTYUNDOBUFFER`/`EM_SETTABSTOPS` 在 `UI::Controls`，`CHARFORMAT2W`/`EM_SETBKGNDCOLOR`/`EM_SETEVENTMASK` 在 `RichEdit`）
     - Shell / COM / 控制台 / 剪贴板：`Win32_System_Com`、`Win32_System_Console`、`Win32_UI_Shell`、`Win32_UI_Shell_Common`、`Win32_System_DataExchange`、`Win32_System_Memory` + **`Win32_System_Ole`**（`CF_UNICODETEXT` 位于 `Win32::System::Ole`，剪贴板复制必需）

4. **Edition 2024 与 Release 优化规范**
   - 采用 `edition = "2024"`、`rust-version = "1.85"`；Edition 2024 默认启用 `unsafe_op_in_unsafe_fn`，因此在 `wndproc` 等 `unsafe fn` 内部调用 Win32/COM API 时必须显式包裹细粒度 `unsafe { ... }` 块并附 `// SAFETY:` 说明，确保 `cargo clippy --all-targets -- -D warnings` 零警告通过。
   - `[profile.release]` 配置 `opt-level = 3`、`lto = "thin"`、`codegen-units = 1`、`strip = true`，剥离调试符号并消除跨 crate 死代码，有效降低最终二进制尺寸与页面映射内存（不默认设置 `panic = "abort"` 以保留 `--release` 测试可诊断性）。

---

## 3. 参考产品借鉴与取舍决策矩阵

| 参考对象 | 核心借鉴项（纳入 MDReader） | 明确剔除项（不引入 MDReader） |
|---|---|---|
| **`https://mdfiler.com/`** | 1. **即开即读无门槛心智**：启动即进入干净排版正文，无广告/注册弹窗。<br>2. **居中限宽呼吸感**：单栏居中版心（默认 `760px`）+ 四周留白（`40px`）+ `1.65` 行高。<br>3. **低眩光 WCAG AAA 配色**：柔和暖白纸面与深灰蓝暗色背景，避免纯白/纯黑刺眼对比。<br>4. **拖拽直接打开**：支持拖入本地 `.md` 文件无缝切换阅读。 | 1. Web / 浏览器 DOM 运行时开销。<br>2. 在线云同步、GitHub 登录与账号体系。<br>3. 任何悬浮推广条与网络依赖。 |
| **`https://github.com/yakoye/Shazao-markdown`** | 1. **纯 Win32 + Direct2D/DirectWrite + WIC 技术路线**：抛弃 WebView2 换取极低单进程内存。<br>2. **视口裁剪与 LRU 布局缓存**：仅维护可视区域周围 `IDWriteTextLayout`，大文档不爆内存。<br>3. **完整 GFM 要素渲染**：标题层级、嵌套/任务复选框、引用竖线、GFM 表格、围栏代码词法着色、本地图片。<br>4. **便携优先配置文件思路**：优先识别同级 `config.json`，其次用户目录。<br>5. **阅读态锁定与源码修改隔离**。 | 1. 重型 IDE 工作台（左侧文件树、多标签栏、大纲栏、收藏夹、文件夹全文搜索）。<br>2. 所见即所得混合编辑态、打字机模式、专注遮罩。<br>3. 复杂多级图形化设置中心与多语言配置 UI。<br>4. Word/EPUB/PDF 导入导出与重型第三方公式/图表引擎。 |
| **`smartpdf`（SumatraPDF 设置范式）** | 1. **主界面零设置面板**：不设 Options/Preferences 对话框、不设设置向导。<br>2. **单一文本配置文件统管全部深度偏好**：深度用户通过编辑本地配置文件 + 重启或热重载快捷键生效。<br>3. **极致容错回退**：配置缺失、语法损坏或值越界时静默安全回退，永不阻断阅读主流程。 | 1. SumatraPDF 自研类 INI 专有配置语法（严格按需求 R3 采用标准 JSON `config.json`）。 |

---

## 4. 分层架构、目录结构与公开接口契约

### 4.1 工程目录结构与并行轨道边界

```text
c:\Users\LIBO\Desktop\MDReader\
├── IMPLEMENTATION_PLAN.md          # M1 实施方案蓝图（本文档）
├── Cargo.toml                      # Rust MSVC 工程与精确依赖配置
├── Cargo.lock                      # Cargo 依赖锁定文件
├── assets/
│   └── mdreader.ico                # [M5] 应用与 .md 文件关联图标资源
├── scripts/
│   ├── register-md-assoc.ps1       # [M3] 一键注册 HKCU .md 打开方式脚本
│   ├── unregister-md-assoc.ps1     # [M3] 一键清理 HKCU 关联脚本
│   └── measure-memory.ps1          # [M6] Windows Working Set 内存基准采样脚本
├── src/
│   ├── lib.rs                      # [M2 起] 导出 core/platform/ui 供集成测试白盒调用
│   ├── main.rs                     # 入口、DPI/COM 初始化、CLI 分发、消息循环
│   ├── core/
│   │   ├── mod.rs                  # 无 GUI 核心层根模块
│   │   ├── config.rs               # [M2] 3层路径、JSON 解析、逐字段回退、热重载
│   │   ├── storage.rs              # [M2] BOM/EOL 嗅探与 ReplaceFileW 原子写回
│   │   ├── markdown.rs             # [M2] pulldown-cmark 流式块模型与表格/任务列表
│   │   └── highlight.rs            # [M2] 轻量状态机多语言代码词法高亮
│   ├── platform/
│   │   ├── mod.rs                  # Windows 平台集成层根模块
│   │   ├── cli.rs                  # [M3] OsString 命令行解析
│   │   └── assoc.rs                # [M3] HKCU 四组注册表写入/清理与 SHChangeNotify
│   └── ui/
│       ├── mod.rs                  # 原生 Win32/D2D UI 层根模块（AppMode 与控件 ID 契约）
│       ├── theme.rs                # [M4] Light / Dark / System 调色板与颜色解析
│       ├── reader_view.rs          # [M4] Direct2D/DirectWrite/WIC 阅读引擎、划词、查找、WM_PRINTCLIENT
│       ├── editor_view.rs          # [M5] Msftedit.dll (RICHEDIT50W) 居中纯文本编辑区
│       └── window.rs               # [M5] 极简主窗口、状态机流转、热重载、拖放与几何记忆
└── tests/
    ├── integration_core.rs         # 核心与 CLI 集成测试
    └── e2e/                        # 【严格禁区】由并行 E2E Testing Track 独占，M1–M5 严禁触碰
```

> **并行协作红线**：
> 1. **严禁触碰 `tests/e2e/`**：M1–M5 实现轨道不得在 `c:\Users\LIBO\Desktop\MDReader\tests\e2e\` 下创建、修改或删除任何文件。
> 2. **严禁污染仓库**：不得在工作目录创建 `.agents/` 目录、代理状态文件或 `.cargo/config.toml`。

### 4.2 四层依赖与职责边界

1. **`src/main.rs`（协调与入口层）**：负责初始化 Per-Monitor V2 DPI 感知与 COM 单线程套间（STA），调用 `platform::cli` 分发动作，并协调 `core` 与 `ui` 启动主窗口消息循环。
2. **`src/platform/`（OS 集成层）**：仅依赖标准库与 Windows 注册表/Shell API，不依赖 `ui` 渲染细节。
3. **`src/core/`（纯逻辑无 GUI 核心层）**：严禁依赖任何 Direct2D/DirectWrite/Win32 窗口控件句柄，确保所有配置、原子存储与 Markdown 解析逻辑可在 `cargo test` 下 100% 离线高速验证。
4. **`src/ui/`（原生 Win32 + D2D + RichEdit 表现层）**：单向消费 `core` 产出的配置、文档编码元信息与 `ParsedDocument`，所有文件读写必须经由 `core::storage` 完成，禁止 UI 层绕过核心层直接写盘。

### 4.3 核心数据流向图

```text
[启动流]
  CLI 参数 (OsString)
    └─▶ platform::cli::parse_args()
          ├─ --register / --unregister / --help ──▶ 无窗口执行并返回退出码
          └─ OpenWindow(file_path, config_path)
                ├─▶ core::config::resolve_config_path() + load_config()
                ├─▶ core::storage::load_document()       [若有文件参数]
                ├─▶ core::markdown::parse_markdown()
                └─▶ ui::window (默认 Reading) ──▶ ui::reader_view (D2D/DWrite/WIC)

[编辑保存自动回读流]
  Reading --[点击"编辑" / Ctrl+E / F2]--> Editing (隐藏 ReaderView, 聚焦 RICHEDIT50W)
    └─▶ 用户编辑 (EN_CHANGE -> dirty = true, 标题追加 "*")
    └─▶ 点击"保存并阅读" / Ctrl+S
          └─▶ core::storage::save_document_atomic(path, text, meta)
                ├─ 失败 ──▶ 保持 Editing 与 dirty=true, 弹窗提示错误 (数据零丢失)
                └─ 成功 ──▶ 置 dirty=false ──▶ core::markdown::parse_markdown()
                              └─▶ 刷新 ReaderView 缓存 ──▶ 自动切回 Reading 展示最新文档
```

### 4.4 全量公开接口契约（对齐 `PROJECT.md § 5`）

#### (1) `core::config`
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode { Light, Dark, System }

/// 自定义颜色覆盖配置（JSON `colors` 子对象键名固定为 camelCase：
/// `background`, `text`, `codeBackground`, `quoteBackground`, `link`, `border`）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CustomColors {
    pub background: Option<String>,
    pub text: Option<String>,
    pub code_background: Option<String>,
    pub quote_background: Option<String>,
    pub link: Option<String>,
    pub border: Option<String>,
}

/// 统一采用 `#[serde(rename_all = "camelCase")]` 对齐 `config.json` 键名，
/// 并可对各字段附加 snake_case `alias` 以兼顾人工手写配置。
#[derive(Debug, Clone, PartialEq)]
pub struct AppConfig {
    pub theme: ThemeMode,
    pub font_family: String,
    pub mono_font_family: String,
    pub font_size: f32,
    pub line_height: f32,
    pub max_width: u32,
    pub page_padding: u32,
    pub editor_font_size: f32,
    pub editor_line_height: f32,
    pub editor_tab_size: u8,
    pub show_line_numbers: bool,
    pub word_wrap: bool,
    pub remember_window_geometry: bool,
    pub window_width: u32,
    pub window_height: u32,
    pub watch_file_changes: bool,
    pub colors: Option<CustomColors>,
}

pub struct ConfigLoadResult {
    pub config: AppConfig,
    pub source_path: Option<PathBuf>,
    pub warning: Option<String>,
}

pub fn resolve_config_path(cli_override: Option<&Path>, exe_dir: &Path) -> PathBuf;
pub fn load_config(path: Option<&Path>) -> ConfigLoadResult;
```

#### (2) `core::storage`
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding { Lf, Crlf }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentEncodingMeta {
    pub has_utf8_bom: bool,
    pub line_ending: LineEnding,
    pub was_lossy_utf8: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedDocument {
    pub path: PathBuf,
    pub text: String,
    pub meta: DocumentEncodingMeta,
}

#[derive(Debug)]
pub enum StorageError { Io(std::io::Error), InvalidPath(String) }

pub fn load_document(path: &Path) -> Result<LoadedDocument, StorageError>;
pub fn save_document_atomic(path: &Path, text: &str, meta: &DocumentEncodingMeta) -> Result<(), StorageError>;
```

#### (3) `core::markdown`
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableAlign { None, Left, Center, Right }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind { Plain, Keyword, String, Comment, Number, TypeOrFn }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RichSpan {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link_url: Option<String>,
    pub highlight: Option<TokenKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Heading { level: u8 },
    Paragraph,
    CodeBlock { language: Option<String> },
    BlockQuote,
    ListItem { ordered: bool, index: Option<u64>, depth: u16, task: Option<bool> },
    Table { alignments: Vec<TableAlign>, rows: Vec<Vec<Vec<RichSpan>>> },
    ThematicBreak,
    Image { url: String, alt: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocBlock { pub kind: BlockKind, pub spans: Vec<RichSpan> }

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedDocument { pub blocks: Vec<DocBlock> }

pub fn parse_markdown(input: &str) -> ParsedDocument;
```

#### (4) `platform::cli` & `platform::assoc`
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliAction {
    OpenWindow { file_path: Option<PathBuf>, config_path: Option<PathBuf> },
    RegisterAssoc,
    UnregisterAssoc,
    ShowHelp,
}

pub fn parse_args<I: IntoIterator<Item = OsString>>(args: I) -> Result<CliAction, CliError>;
// HKCU ProgID 固定为 "MDReader.Document"
pub fn register_md_association(exe_path: &Path) -> Result<(), AssocError>;
pub fn unregister_md_association() -> Result<(), AssocError>;
```

#### (5) `ui` 控件 ID、状态与窗口标题契约（UIA 自动化可测性 + 后台截图）
- **核心状态枚举**：`pub enum AppMode { Reading, Editing }`（默认 `Reading`）。
- **固定控件 ID 与标准 Win32 控件契约（顶部 36px 极简工具栏，无任何设置面板/弹窗/树控件）**：
  - 按钮均采用标准 Win32 **`"BUTTON"`** 窗口类及固定中文文案：
    - `IDC_MODE_TOGGLE = 1001`（阅读态文案 `"编辑"`，修改态文案 `"保存并阅读"`）
    - `IDC_EDIT_CANCEL = 1002`（仅修改态可见，文案 `"取消"`）
    - `IDC_FILE_OPEN = 1003`（文案 `"打开"`）
  - `IDC_EDITOR = 1004`（`Msftedit.dll` `RICHEDIT50W` 控件，原生支持 UIA `TextPattern`/`ValuePattern` 与中文 IME）
- **固定窗口标题格式**：
  - 阅读模式：`"<filename>[*] - 阅读模式 - MDReader"`（未打开文件时 `<filename>` 为 `"未命名"`）
  - 修改模式：`"<filename>[*] - 修改模式 - MDReader"`（`dirty == true` 时在 `<filename>` 后紧跟 `*`）
- **后台截图渲染契约（`WM_PRINTCLIENT`）**：
  - `reader_view` 必须处理 `WM_PRINTCLIENT`（借助 `ID2D1DCRenderTarget` 将当前文档内容完整绘制到传入的 `HDC`），确保在无前景桌面会话下通过 `PrintWindow(hwnd, hdc, PW_RENDERFULLCONTENT)` 截图完整呈现而非纯白屏。

---

## 5. 模式流转状态机与极简交互设计

### 5.1 核心状态模型

主窗口状态机围绕三元组 `(mode: AppMode, dirty: bool, current_doc: Option<LoadedDocument>)` 运行：

```text
                       启动 (有路径 / 拖放 / 无参空态)
                                    │
                                    ▼
                          ┌───────────────────┐
                          │   AppMode::Reading │ ◀── 始终为默认初始模式
                          │  (D2D 阅读画布可见) │     dirty = false
                          └───────────────────┘
                             │            ▲
      "编辑" / Ctrl+E / F2   │            │  保存成功 (自动切回)
      (填充全文, dirty=false) │            │  或无脏改动点击"取消"/Esc
                             ▼            │
                          ┌───────────────────┐
                          │  AppMode::Editing │
                          │ (RICHEDIT50W 可见) │──▶ 保存失败: 弹窗报错，
                          └───────────────────┘    保留修改并留守 Editing!
                             │            ▲
                             └─ EN_CHANGE ┘ (排除程序SetWindowTextW初始化事件)
                                dirty = true, 标题追加 "*"
```

### 5.2 关键流转与异常保护细则

1. **进入编辑态（`Reading -> Editing`）**：
   - 隐藏 `ReaderView`，同步计算与 `maxWidth`/`pagePadding` 对齐的 `EM_SETRECT` 居中编辑内边距，同步当前主题背景色（`EM_SETBKGNDCOLOR`）与等宽字体/字号/Tab 宽度（`CHARFORMAT2W` / `EM_SETTABSTOPS`）。
   - 使用布尔守卫 `suppress_change_event = true` 包裹 `SetWindowTextW` 与 `EM_SETMODIFY(0)`、`EM_EMPTYUNDOBUFFER`，确保刚切入编辑模式时**绝不误触发**脏标记 `*`。
   - 显式调用 `SetFocus(editor_hwnd)`，使用户可直接键盘录入或调用中文 IME。
2. **保存并自动回读（`Editing` 下按 `Ctrl+S` 或点击 `"保存并阅读"`）**：
   - 若当前尚未绑定磁盘路径（无参空态新建编辑），先呼出系统保存对话框确定目标 `.md` 路径；若取消则留守 `Editing`。
   - 从 `RICHEDIT50W` 提取纯文本，规范化换行符以匹配 `DocumentEncodingMeta.line_ending`，调用 `core::storage::save_document_atomic` 执行同卷临时文件创建、写入、**`FlushFileBuffers`** 刷盘，再调用 **`ReplaceFileW`**（若目标为新建文件或卷不支持则回退 `MoveFileExW(..., MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)`）。
   - **写入成功**：清除 `dirty = false`，更新内存 `LoadedDocument.text`，立即执行 `core::markdown::parse_markdown` 更新 `ReaderView` 渲染数据与滚动位置保护，隐藏 `EditorView`、显示 `ReaderView`，**自动切回 `AppMode::Reading`** 并更新标题为 `"<filename> - 阅读模式 - MDReader"`。
   - **写入失败**（目标只读、外部进程独占锁、磁盘空间不足、父目录被移除）：**严禁切回阅读模式**，保持 `AppMode::Editing` 与 `dirty = true`，清理残留临时文件（确保原文件毫发无损），弹出明确 Win32 错误提示，允许用户解除占用或另存重试。
3. **脏状态离开保护（`Editing` 点击 `"取消"`/`Esc`、`Ctrl+O`、`WM_DROPFILES`、`WM_CLOSE`）**：
   - `dirty == false`：直接静默切回 `Reading` / 载入新文件 / 关闭窗口。
   - `dirty == true`：弹出 `MessageBoxW(MB_YESNOCANCEL | MB_ICONWARNING)`（“是否保存对当前文档的修改？”）：选“是”走完整原子保存流程并仅在成功后继续切换/关闭；选“否”放弃编辑并恢复原文档视图；选“取消”继续留守 `Editing`。
4. **0 字节文档合法性保护**：
   - 支持打开 0 字节 `.md` 文件并呈现整洁空阅读页；在修改模式下清空全部文本后按 `Ctrl+S`，允许合法原子写回 0 字节文件并顺利切回阅读模式。

### 5.3 GUI 子系统演进策略（M1 → M3 → M5）

- **M1 阶段**：保留标准控制台 `fn main()`（不启用 `windows_subsystem = "windows"`），确保 `cargo run -- --help` 与 `tests/integration_core.rs` 可直接非阻塞捕获 stdout 与退出码。
- **M3 阶段**：在 `platform::cli` 中封装终端输出辅助函数：先检测 `GetStdHandle(STD_OUTPUT_HANDLE)` 是否已有效（如 PowerShell 重定向管道），仅在无有效句柄时调用 `AttachConsole(ATTACH_PARENT_PROCESS)`。
- **M5 阶段**：在 `src/main.rs` 顶部无条件启用 `#![windows_subsystem = "windows"]`，确保从资源管理器双击 `.md` 启动时（无论 debug 还是 release）**零控制台黑框闪烁**，同时 `--help`/`--register`/`--unregister` 依然通过父控制台附加正常输出提示。

---

## 6. Windows `.md` 打开方式集成与单一本地 JSON 配置设计

### 6.1 Windows `.md` 文件关联机制（HKCU 无提权规范）

为保证普通用户无需管理员权限即可一键注册与干净卸载，**全部注册表操作严格限定在 `HKEY_CURRENT_USER`（HKCU）**，且**严禁读写受 Windows 保护哈希校验的 `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.md\UserChoice` 键**。

#### (1) 四组 HKCU 精确注册表键值清单

| 分组 | 注册表路径 | 值名 | 值类型 | 值内容 / 规范 |
|---|---|---|---|---|
| **1. ProgID 定义** | `HKCU\Software\Classes\MDReader.Document` | `(Default)` | `REG_SZ` | `"Markdown Document"` |
| | `HKCU\Software\Classes\MDReader.Document` | `FriendlyTypeName` | `REG_SZ` | `"Markdown Document"` |
| | `HKCU\Software\Classes\MDReader.Document\DefaultIcon` | `(Default)` | `REG_SZ` | `"<ExePath>,0"` |
| | `HKCU\Software\Classes\MDReader.Document\shell\open\command` | `(Default)` | `REG_SZ` | `"\"<ExePath>\" \"%1\""` |
| **2. 扩展名候选** | `HKCU\Software\Classes\.md\OpenWithProgids` | `MDReader.Document` | `REG_NONE` | 0 字节空值（纳入右键“打开方式”列表） |
| | `HKCU\Software\Classes\.markdown\OpenWithProgids` | `MDReader.Document` | `REG_NONE` | 0 字节空值 |
| **3. Applications 元数据** | `HKCU\Software\Classes\Applications\MDReader.exe` | `FriendlyAppName` | `REG_SZ` | `"MDReader"` |
| | `HKCU\Software\Classes\Applications\MDReader.exe\DefaultIcon` | `(Default)` | `REG_SZ` | `"<ExePath>,0"` |
| | `HKCU\Software\Classes\Applications\MDReader.exe\SupportedTypes` | `.md` / `.markdown` | `REG_SZ` | `""`（空字符串） |
| | `HKCU\Software\Classes\Applications\MDReader.exe\shell\open\command` | `(Default)` | `REG_SZ` | `"\"<ExePath>\" \"%1\""` |
| **4. 系统默认应用注册** | `HKCU\Software\RegisteredApplications` | `MDReader` | `REG_SZ` | `"Software\\MDReader\\Capabilities"` |
| | `HKCU\Software\MDReader\Capabilities` | `ApplicationName` | `REG_SZ` | `"MDReader"` |
| | `HKCU\Software\MDReader\Capabilities` | `ApplicationDescription` | `REG_SZ` | `"纯粹低内存 Windows Markdown 阅读器"` |
| | `HKCU\Software\MDReader\Capabilities\FileAssociations` | `.md` / `.markdown` | `REG_SZ` | `"MDReader.Document"` |

#### (2) 路径引号、Shell 通知与双交付通道
- **双引号强制规范**：命令行模板必须严格为 `"\"<ExePath>\" \"%1\""`，确保路径中含有空格、中文、括号或特殊符号（如 `C:\My Docs\笔记 (v1).md`）时能完整作为单个 `argv[1]` 传递。
- **Shell 刷新**：注册或卸载完成后立即调用 `SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None)` 通知资源管理器实时刷新右键菜单与图标缓存。
- **精确卸载边界**：`--unregister` 仅删除 `MDReader.Document`、`Applications\MDReader.exe`、`Software\MDReader` 子树以及 `OpenWithProgids` 下的 `MDReader.Document` 值、`RegisteredApplications` 下的 `MDReader` 值；**绝不删除 `.md` 键本身或其他软件的 ProgID**。
- **双交付通道**：二进制内建 `MDReader.exe --register` / `--unregister`，同时在 `scripts/` 提供独立可审计脚本 `register-md-assoc.ps1` 与 `unregister-md-assoc.ps1`。

### 6.2 单一本地 JSON 配置设计（`config.json`）

#### (1) 三层查找优先级
1. **CLI 显式指定（最高优先级）**：`MDReader.exe --config <path>`。若显式指定路径不存在或 JSON 损坏，不崩溃，回退默认配置并保留非阻塞诊断信息。
2. **便携同级目录**：可执行文件同级目录 `./config.json`（若存在则优先于用户目录）。
3. **系统用户目录**：`%APPDATA%\MDReader\config.json`（默认标准落盘路径）。

#### (2) 完整 Schema（16 核心字段 + 可选 `colors` 对象，统一 `camelCase`）

| JSON 字段名 | Rust 类型 | 默认值 | 校验规则 / 合法范围 | 非法 / 越界处理 |
|---|---|---|---|---|
| `theme` | `ThemeMode` | `"light"` | `"light" \| "dark" \| "system"`（忽略大小写） | 回退 `"light"` |
| `fontFamily` | `String` | `"Microsoft YaHei UI, Segoe UI, sans-serif"` | 非空字符串，trim 后长度 `1..=256` | 回退默认值 |
| `monoFontFamily` | `String` | `"Cascadia Code, Consolas, monospace"` | 非空字符串，trim 后长度 `1..=256` | 回退默认值 |
| `fontSize` | `f32` | `16.0` | 有限浮点数 `[12.0, 32.0]` | 回退 `16.0` |
| `lineHeight` | `f32` | `1.65` | 有限浮点数 `[1.2, 2.5]` | 回退 `1.65` |
| `maxWidth` | `u32` | `760` | 整数 `[480, 1400]` | 回退 `760` |
| `pagePadding` | `u32` | `40` | 整数 `[12, 120]` | 回退 `40` |
| `editorFontSize` | `f32` | `15.0` | 有限浮点数 `[12.0, 28.0]` | 回退 `15.0` |
| `editorLineHeight` | `f32` | `1.6` | 有限浮点数 `[1.2, 2.5]` | 回退 `1.6` |
| `editorTabSize` | `u8` | `4` | 整数 `[2, 8]` | 回退 `4` |
| `showLineNumbers` | `bool` | `true` | 严格布尔值 | 回退 `true` |
| `wordWrap` | `bool` | `true` | 严格布尔值 | 回退 `true` |
| `rememberWindowGeometry` | `bool` | `true` | 严格布尔值 | 回退 `true` |
| `windowWidth` | `u32` | `960` | 整数 `[400, 3840]` | 回退 `960` |
| `windowHeight` | `u32` | `760` | 整数 `[300, 2160]` | 回退 `760` |
| `watchFileChanges` | `bool` | `false` | 严格布尔值 | 回退 `false` |
| `colors` *(可选)* | `Option<CustomColors>` | `None` | 对象内可选键：`background`, `text`, `codeBackground`, `quoteBackground`, `link`, `border`；每项必须为 `#RRGGBB` 6 位十六进制 | 逐色校验，非法单色回退当前主题默认色 |

#### (3) 容错解析、热重载与几何保存安全保护
- **禁止整结构一刀切反序列化**：不得直接使用 `serde_json::from_str::<AppConfig>`（否则单个字段类型错误会导致整份配置全部失效）。必须先解析为 `serde_json::Value`：
  - 若 JSON 语法非法、文件为空或根节点非 `Value::Object`，整体安全回退 `AppConfig::default()` 并写入 `warning`（标记 `source_is_corrupt = true`）；
  - 若根节点为 `Object`，则**逐字段读取与范围校验**，合法字段即刻生效，坏字段单独回退默认值。
- **热重载机制**：支持修改 JSON 后重启生效、`Ctrl+Shift+R` / `F5` 手动即时重载，以及在 `WM_ACTIVATE` 或 `watchFileChanges = true` 时检测 `config.json` 修改时间变化防抖刷新主题与版心。
- **窗口几何保存防损坏红线**：当 `rememberWindowGeometry == true` 且窗口关闭时，仅当配置文件此前解析合法（非损坏状态）且具备写入权限时，才保留用户其余字段并原子更新 `windowWidth`/`windowHeight`；**严禁因保存窗口几何而覆写或清空用户损坏/只读的配置文件**。

---

## 7. 自动化验证、Working Set 内存基线与独立阅读观感验收计划

### 7.1 四层自动化与实测验证体系

| 验证层级 | 覆盖范围 | 执行方式与核心校验点 |
|---|---|---|
| **L1：单元测试（`cargo test`）** | `core` + `platform` | 1. `config`：3 层优先级、缺失文件、非法 JSON、非 Object 根、`NaN`/`Infinity`/越界数值、非法 `colors`、`--config` 覆盖。<br>2. `storage`：BOM 与无 BOM、LF 与 CRLF 保留、0 字节写入、只读目标原子失败回退（原文件完好且临时文件清除）。<br>3. `markdown`：H1–H6、段落、粗斜体/删除线、嵌套与任务列表、GFM 表格对齐、引用、代码词法高亮、分割线、图片。<br>4. `cli` & `assoc`：空格/中文/括号路径、`--register`/`--unregister` 键构建与清理逻辑。 |
| **L2：集成测试与 HKCU 实测** | `tests/integration_core.rs` + PS1 | 1. 核心闭环集成：读取 `.md` -> 模拟编辑 -> `save_document_atomic` -> 重解析与编码验证。<br>2. 实机运行 `MDReader.exe --register` 与 `scripts/register-md-assoc.ps1`，校验 HKCU 4 组键值与 `"\"<ExePath>\" \"%1\""` 引号格式，随后 `--unregister` 精确还原。 |
| **L3：Windows UIA 端到端闭环** | 并行 E2E Track & M6 联调 | 1. 命令行传入 `.md` 路径默认进入阅读态（`IDC_EDITOR` 不可见，`IDC_MODE_TOGGLE` 显示 `"编辑"`）。<br>2. 触发 `"编辑"` / `Ctrl+E` 进入修改态（`IDC_EDITOR` 可见聚焦），修改正文后标题出现 `*`。<br>3. 触发 `"保存并阅读"` / `Ctrl+S` 后，验证磁盘文件更新、界面自动切回阅读模式并展示新内容。<br>4. `--config` 指向自定义 JSON 后重启或 `Ctrl+Shift+R` 生效；损坏 JSON 启动无崩溃正常阅读；确认界面无复杂设置面板；校验 `WM_PRINTCLIENT` / `PrintWindow(..., PW_RENDERFULLCONTENT)` 截图非白屏。 |
| **L4：内存实测与独立体验评审** | `scripts/measure-memory.ps1` + 独立评审量表 | 在 Windows 11 实机采集空载冷启动与典型 `.md` 进程 `WorkingSet64`；由独立评审智能体对真实渲染截图/窗口执行 100 分观感检查。 |

### 7.2 Working Set 内存基准采集口径（`scripts/measure-memory.ps1`）

1. **空载冷启动基线**：无参数启动 `MDReader.exe`，窗口就绪稳定等待 3 秒后采样 `WorkingSet64` 与 `PrivateMemorySize64`：
   - **验收门槛**：`WorkingSet64 < 20 MB`（预期区间 `8–15 MB`），且进程树中**零 `msedgewebview2.exe`** 子进程。
2. **典型 Markdown 文档基线**：加载一份包含 H1–H6、多级列表、任务列表、宽 GFM 表格、多段围栏高亮代码块及本地图片的典型 `.md` 文档，稳定 3 秒后采样：
   - **验收门槛**：`WorkingSet64 < 30 MB`（预期区间 `12–22 MB`）。

### 7.3 独立阅读观感 100 分验收量表（对标 `mdfiler.com`）

#### (1) 一票否决项（任一项失败即判定不通过）
1. **无复杂设置干扰**：主界面不存在图形化设置面板、多选项卡设置对话框或与阅读无关的工具栏/侧边栏。
2. **闭环完整性**：默认进入阅读模式 -> 一键切入修改模式 -> 修改保存后自动返回阅读模式并显示最新文本。
3. **文字与渲染健康度**：中文、英文及代码块混排清晰无乱码/方块缺字，图片/表格不导致崩溃或脚本弹窗。

#### (2) 五维度量化评分（总分 100 分，及格线 ≥ 85 分）

| 评估维度 | 分值 | 核心检查细则 |
|---|---:|---|
| **1. 界面极简度与零干扰** | 25 | 正文阅读视口面积占比 ≥ 90%；阅读态可见交互按钮 ≤ 4 个；无多余状态栏/树栏。 |
| **2. 版心排版与留白呼吸感** | 25 | 正文居中限宽（默认 ~760px）、左右对称留白充足（~40px）、段落/标题间距与 1.65 行高疏密得当。 |
| **3. 色彩与对比度舒适度** | 15 | 亮色/暗色主题正文与背景对比度 ≥ 7:1（WCAG AAA）；暗色模式下无刺眼纯白块或默认白底控件残留。 |
| **4. Markdown 元素渲染清晰度** | 25 | 覆盖 H1–H6、粗斜体、删除线、有序/无序/任务列表、引用块、GFM 表格、行内与围栏代码高亮、链接、图片、分割线共 ≥ 10 类元素，宽表/长代码块横向溢出受控不撑破版心。 |
| **5. 模式切换的视觉连续性** | 10 | 阅读 ↔ 修改切换时版心宽度、主题背景与字体尺度衔接自然，保存后自动回到阅读模式无明显闪烁或位置错乱。 |

### 7.4 里程碑落地路线图

- **M1 (`docs-and-scaffold`)**：完成 `IMPLEMENTATION_PLAN.md` 与纯净可编译 Rust 工程骨架（当前里程碑）。
- **M2 (`mdreader-core`)**：实现 `config.rs`、`storage.rs`、`markdown.rs`、`highlight.rs`、`src/lib.rs` 及核心单元测试。
- **M3 (`win32-platform`)**：实现 `cli.rs`、`assoc.rs`、`scripts/register-md-assoc.ps1`、`unregister-md-assoc.ps1` 与 HKCU 注册实测。
- **M4 (`d2d-reader-engine`)**：实现 `theme.rs` 与 `reader_view.rs`（Direct2D/DirectWrite/WIC 全元素渲染、LRU 布局、划词 `Ctrl+C`、链接点击、`Ctrl+F`）。
- **M5 (`app-shell-and-editor`)**：实现 `editor_view.rs`（`Msftedit.dll`）、`window.rs`、GUI 子系统切换、托盘/拖放、`Ctrl+O`、热重载与几何记忆。
- **M6 (`final-milestone`)**：按 Tier 1→4 跑通 E2E 套件、Tier-5 白盒加固、实测 Working Set 内存并完成独立观感量表评审。
