---
trigger: model_decision
description: 修改底层 Win32/Direct2D 渲染、编辑区、存储原子写盘、JSON 配置、文件关联或模块架构边界时加载
---

# MDReader 系统架构与关键模块规范

本文件按需加载。涉及底层渲染、存储、配置、文件关联与模块边界调整时必须严格遵照。

## 一、技术选型与第一性优先级

1. **核心优先级：`内存 > UI 美化 > 数据安全`**
   - 纯原生 Rust (MSVC) + Win32 API (`windows` crate) + Direct2D / DirectWrite / WIC + `pulldown-cmark` + `Msftedit.dll` (`RICHEDIT50W`) + `serde_json` 单进程架构。
   - **严禁**引入 Electron、WebView2 (`msedgewebview2.exe`)、CEF 或任何重型跨平台 Web/脚本运行时。
2. **极低冷启动与运行时内存控制**
   - 阅读画布采用软件双缓冲 `ID2D1DCRenderTarget`（`D2D1_RENDER_TARGET_TYPE_SOFTWARE` + `D2D1_ALPHA_MODE_IGNORE`）配合 32bpp DIBSection，既避免加载数十 MB GPU 3D 驱动工作集，又保留 Direct2D 矢量抗锯齿与 DirectWrite ClearType 清晰渲染。
   - 非冷启动必需的系统组件（`shell32.dll`、`comdlg32.dll`、`uiautomationcore.dll`、`Msftedit.dll`）必须采用运行时按需懒加载，压低初始 Working Set 与进程模块数量。

## 二、模块分层与单向依赖

1. **三层单向依赖架构**：`core` ← `platform` / `ui`
   - `src/core/`（`config`、`storage`、`markdown`、`highlight`）：严禁依赖 `ui` 模块或任何窗口控件/Direct2D 句柄，保证可在 `cargo test` 下完全离线单测。
   - `src/platform/`（`cli`、`assoc`）：独立于 `ui` 渲染细节，负责命令行 `OsString` 无损解析与 HKCU 文件关联。
   - `src/ui/`（`theme`、`uia`、`reader_view`、`editor_view`、`window`）：单向消费 `core` 产出的配置、编码元信息与解析块模型。
2. **严禁绕过存储层写盘**
   - `ui` 层所有文件落盘操作必须统一经由 `core::storage::save_document_atomic` 执行，禁止在 UI 消息回调中直接裸写目标文件。

## 三、阅读 ↔ 修改模式状态机与数据安全

1. **模式流转与防误触守卫**
   - 启动默认进入 `AppMode::Reading`；点击「编辑」或按 `Ctrl+E`/`F2` 切入 `AppMode::Editing`；保存成功后必须**立即自动切回 `AppMode::Reading`** 并展示最新渲染内容。
   - 初始化编辑区填充文本时，必须用 `suppress_change_event = true` 包裹 `SetWindowTextW`、`EM_SETMODIFY(0)` 与 `EM_EMPTYUNDOBUFFER`，严禁刚进入编辑态就误触发未保存 `*` 标记。
2. **原子落盘与编码无损保留**
   - 读取时必须嗅探并记录原始 UTF-8 BOM 与主换行风格（LF / CRLF），写盘时无损还原，且严禁在 CRLF 模式下产生 `\r\r\n` 双回车。
   - 写入必须走“同目录唯一临时文件 + `FlushFileBuffers` + `ReplaceFileW`（目标不存在时回退 `MoveFileExW`）”，借助 RAII 守卫确保失败时清理临时文件且原文件毫发无损。
3. **保存失败熔断与脏状态保护**
   - 写回失败（只读、占用锁、权限不足）或编辑既有文件期间原文件已被外部删除时，必须弹出明确错误提示并**熔断留守 `AppMode::Editing`**，完整保留用户编辑文本与脏状态。
   - 带有未保存改动切换文件、取消编辑或关闭窗口时，必须通过确认对话框（保存 / 放弃 / 取消）拦截。

## 四、极简界面与单一 `config.json` 配置规范

1. **极简无设置界面（smartpdf 哲学）**
   - 窗口严禁出现图形化设置面板、设置对话框、菜单树或侧边文件树。所有外观与行为偏好统一收敛于本地 `config.json`。
2. **三层路径优先级与容错回退**
   - 路径查找优先级：`--config <路径>` > 可执行文件同级 `./config.json` > `%APPDATA%\MDReader\config.json`。
   - 严禁直接使用 `serde_json::from_str::<AppConfig>` 一刀切反序列化；必须先解析为 `serde_json::Value` 再逐字段边界校验，坏字段单独回退默认值，整文件损坏时安全回退默认且标记 `source_is_corrupt = true`。
3. **窗口几何写回红线**
   - 仅当配置解析合法且有写权限时才允许在关闭窗口时局部更新 `windowWidth` / `windowHeight`（保留用户其他字段与未知键）；严禁因保存窗口几何而覆写损坏或只读的配置文件。

## 五、Direct2D 双缓冲、UIA 与 Windows `.md` 关联红线

1. **同源绘制与内存有界缓存**
   - `reader_view` 的 `WM_PAINT`、`WM_PRINT`、`WM_PRINTCLIENT` 必须共用同一双缓冲绘制管线，彻底杜绝离屏 `PrintWindow` 截屏白屏。
   - `IDWriteTextLayout`（上限 96）与 WIC 解码图片（上限 16、单边最大 1440px）必须使用有界 LRU 缓存，绘制时按视口二分裁剪。
   - 禁止跨 RenderTarget 长期持有或共享 `ID2D1SolidColorBrush` / `DrawingEffect`，每帧按当前 `ID2D1RenderTarget` 创建并在 `DrawTextLayout` 后立即解绑 `DrawingEffect`。
2. **UIA 树完整性保护**
   - 标准 Win32 控件（`BUTTON`、`EDIT`、`STATIC`、`RICHEDIT50W`）交由系统原生 UIA 代理处理，禁止挂载破坏兄弟链表的错误 Override Provider；仅自定义画布 `MDReaderCanvas` 通过 `UiaHostProviderFromHwnd` 暴露 `IDC_READER`。
3. **Windows `.md` 文件关联安全边界（HKCU 无提权）**
   - 全部注册表操作严格限定在 `HKEY_CURRENT_USER`（ProgID 固定为 `MDReader.Document`，命令模板严格为 `"\"<AbsExePath>\" \"%1\""`）。
   - 严禁读写受系统哈希保护的 `Explorer\FileExts\.md\UserChoice` 键。
   - `--unregister` 仅精准删除 MDReader 自建子树与自建值名，严禁删除 `.md` 键本身或其他软件的关联项，操作完成后按需调用 `SHChangeNotify(SHCNE_ASSOCCHANGED, ...)`。

## 六、架构变更检查清单

修改上述模块时，必须确认：

- [ ] 是否严格遵守 `core` 无 GUI 依赖及 `ui` 不直接裸写磁盘的单向边界？
- [ ] 冷启动新增依赖是否会引入非必要常驻 DLL 或破坏 `内存 > UI 美化 > 数据安全`？
- [ ] 保存失败或原文件被外部删除时，是否熔断留守 `AppMode::Editing` 且不丢用户编辑内容？
- [ ] 配置解析是否逐字段容错回退，且窗口几何写回是否绝不覆盖损坏/只读配置？
- [ ] Direct2D 绘制是否同步兼容 `WM_PAINT` 与 `WM_PRINTCLIENT`，且 `DrawingEffect` 不跨 RenderTarget 残留？
- [ ] HKCU 文件关联是否严守无 `UserChoice` 触碰与精确卸载零误伤红线？
