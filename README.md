<p align="center">
  <img src="logo.svg" width="96" height="96" alt="MDReader Logo" />
</p>

<h1 align="center">MDReader</h1>

<p align="center">
  纯粹、极低内存的 Windows 桌面端原生 Markdown 阅读与轻量编辑器<br/>
  <code>Rust + Win32 + Direct2D / DirectWrite / WIC + pulldown-cmark + Msftedit.dll</code>
</p>

---

**MDReader** 严格遵循 **`内存 > UI 美化 > 数据安全`** 原则，采用纯原生单进程架构构建，零 Electron / WebView2 膨胀，冷启动即开即读，适合作为 Windows 日常 `.md` / `.markdown` 文件的默认打开方式。

## 核心特性

- **融合式极简自绘标题栏**：取消冗余菜单栏与独立工具栏，采用无边框融合设计；左上角依次放置 **Logo**、**侧边栏折叠/展开图标**，以及随模式自动切换的汉字操作按钮——阅读模式为 **`打开` / `修改` / `关闭`**，编辑模式自动切换为 **`打开` / `保存` / `关闭`**，保留原生窗口拖拽、分屏贴靠与系统控制按钮。
- **左侧目录栏（大纲 + 7 天文件历史）**：顶部双 Tab 一键切换，默认展开 **大纲目录**（`H1`–`H6` 层级缩进、点击即跳、阅读滚动时实时高亮当前章节）；**文件历史**持久化记录最近 7 天打开过的文件，超期自动清理。
- **阅读 ↔ 编辑双向精准滚动同步**：基于 `pulldown-cmark` 源码行号与块内比例插值，结合 `RICHEDIT50W` 逻辑行映射（兼容自动折行），切换模式时精准延续当前阅读或编辑位置。
- **柔和护眼 Direct2D 排版渲染**：对标 [mdfiler.com](https://mdfiler.com/) 暖纸阅读美学（默认 `760px` 居中版心、`1.65` 行高、WCAG AAA 明暗主题自适应），完整支持 GFM 标题、粗斜体/删除线、多级与任务列表、嵌套引用、斑马纹表格、6 色词法高亮代码块、本地图片与水平分割线。
- **原子写盘与数据安全**：编辑模式点击 **保存** 或按 `Ctrl+S` 后，通过同目录临时文件 + `FlushFileBuffers` + `ReplaceFileW` 原子落盘并**自动切回阅读模式**，无损保留 UTF-8 BOM 与 LF/CRLF；写入失败或原文件被外部删除时熔断留守编辑区，完整保护未保存内容。
- **无提权 Windows `.md` 文件关联**：纯 `HKEY_CURRENT_USER`（HKCU）注册，无需管理员权限即可一键加入右键“打开方式”与默认应用候选，支持含空格、中文与括号的路径直接双击打开。

## 快速开始

### 方式一：下载绿色便携版（推荐）

1. 前往本仓库 [Releases](https://github.com/li5bo5/MDReader/releases) 页面下载 `MDReader-v0.1.0-win-x64.zip`。
2. 解压到任意目录后，直接双击 `MDReader.exe` 或将 `.md` 文件拖入窗口即可使用。

### 方式二：从源码编译（Windows MSVC）

```powershell
cargo build --release
```

编译产物位于 `target\release\MDReader.exe`（独立单文件可执行程序，零外部运行时依赖）。

## 注册 `.md` 文件关联（无需管理员权限）

```powershell
# 方式一：运行配套脚本
.\scripts\register-md-assoc.ps1

# 方式二：直接调用程序命令行参数
.\MDReader.exe --register
```

注册完成后，右键任意 `.md` 文件选择 **打开方式 → MDReader**（或勾选“始终使用此应用”设为默认阅读器）。如需干净卸载关联（绝不误伤其他 Markdown 软件）：

```powershell
.\scripts\unregister-md-assoc.ps1
# 或 .\MDReader.exe --unregister
```

## 快捷键一览

| 快捷键 | 作用域 | 功能 |
|---|---|---|
| `Ctrl+E` / `F2` | 全局 | 阅读模式 ↔ 修改模式切换（保持当前滚动进度） |
| `Ctrl+S` | 修改模式 | 原子保存并自动切回阅读模式 |
| `Esc` | 查找栏 / 修改模式 / 阅读模式 | 关闭查找栏；修改模式下确认退回阅读模式；阅读模式下弹窗确认退出软件 |
| `Ctrl+O` | 全局 | 打开本地 `.md` 文件（也支持直接拖入窗口） |
| `Ctrl+F` | 阅读模式 | 唤起页内查找栏（`Enter` / `Shift+Enter` 上下跳转） |
| `Ctrl+C` | 阅读模式 | 复制鼠标划词选中的正文文本 |
| `Ctrl+Shift+R` / `F5` | 全局 | 即时热重载 `config.json` 配置 |
| `PageUp` / `PageDown` / `Home` / `End` | 阅读模式 | 键盘快速翻页与回顶/到底 |

## JSON 配置（`config.json`）

软件不设图形化设置面板，所有外观与排版偏好（明暗主题、字号、字体、版心宽度、自定义配色、文件监听等）均收敛于单一 `config.json`，按以下优先级自动查找（缺失或损坏时逐字段安全回退默认值）：

1. 命令行显式指定：`MDReader.exe --config <路径> <文件.md>`
2. 程序同级便携配置：`./config.json`
3. 当前用户配置目录：`%APPDATA%\MDReader\config.json`

完整配置示例见 [config.example.json](config.example.json)。修改后可直接在窗口内按 `Ctrl+Shift+R` 或 `F5` 热生效。

## 开发与测试

- 完整技术设计文档：[IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md)
- 单元与集成测试：`cargo test`
- E2E 端到端测试套件：`powershell -ExecutionPolicy Bypass -File tests\e2e\run-e2e.ps1 -ExePath target\release\MDReader.exe`
- Windows Working Set 内存基线实测：`powershell -ExecutionPolicy Bypass -File scripts\measure-memory.ps1`
