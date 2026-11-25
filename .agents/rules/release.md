---
trigger: model_decision
description: 发版、发布新版本、版本号、Git 提交、打标、打包、GitHub Release 时加载
---

# MDReader 版本发布与 Git 流程

本文件在“发布新版本”“发版”“发布 X.X”等意图下加载。执行发布前，必须遵守 `core.md` 的讨论与执行边界。

## 一、版本命名规范（SemVer）

1. **版本格式**
   - 正式发布：`主版本号.次版本号.修订号`（如 `0.2.0`、`1.0.0`），对外 Git 标签可标识为 `v<Version>`。
   - 内部调试：`主版本号.次版本号.修订号`（如 `0.1.1`、`0.1.2`）。

2. **升级规则**
   - **主版本号 (Major)**：用户定义。
   - **次版本号 (Minor)**：用户定义。
   - **版本修订号 (Patch / 编译自我编号)**：
     - 仅用于本地构建或编译时内部流水编号，精准追踪二进制产物。
     - 每次代码变更触发编译前，修订号自动增加 `0.0.1`，如 `0.1.0 -> 0.1.1 -> 0.1.2`。
     - 最高递增至 `0.0.99`，即末尾修订号最大为 `99`。达到 99 后保持，提醒用户处理。

3. **版本修改位置**
   - 主工程 `Cargo.toml` 中 `[package]` 段的 `version = "..."`，并同步核对 `assets/mdreader.manifest` 中的 `assemblyIdentity version`（4 段格式 `X.Y.Z.0`）。

## 二、构建与交付铁律

1. **编译前终结原有程序**
   - 重新编译前，必须首先强制终结正在运行的原程序进程：
     ```powershell
     Stop-Process -Name "MDReader" -Force -ErrorAction SilentlyContinue
     ```
   - 彻底释放可执行文件占用，确保新二进制无阻碍覆盖。

2. **拉起与运行交由用户**
   - 构建完成后，交给用户手动拉起程序。
   - 常规交付验收不需要智能体在非交互环境中长期驻留运行程序。
   - 单元测试通过且 `cargo build --release` 达到 **0 警告 0 错误** 即可完成交付。

3. **构建门禁**
   - 执行 `cargo test` 全部通过，且 `cargo build --release` 必须 **0 警告 0 错误**。

4. **打包与构建产物隔离**
   - `target/`、`Publish/`、`*.zip` 发布包及 `reports/` 必须被 `.gitignore` 忽略。

## 三、Git 提交与打标规范

1. **提交信息**
   - 遵循 Conventional Commits：`<type>(<scope>): <subject>`（前缀保留英文，描述与正文使用简体中文）。
   - 示例：
     ```powershell
     git commit -m "chore(release): v0.2.0 - 完成 Direct2D 双缓冲阅读渲染与阅读修改一键闭环"
     ```

2. **打标**
   - 发布版本时必须打附注标签：
     ```powershell
     git tag -a v0.2.0 -m "Release v0.2.0"
     ```

3. **推送顺序**
   - 先推送分支，再推送标签：
     ```powershell
     git push origin main
     git push origin v<Version>
     ```

## 四、自动化编译打包

自动化编译打包流程，共四步：

```text
[步骤 1: 版本确定] -> [步骤 2: 本地构建与验证] -> [步骤 3: 自动化打包] -> [步骤 4: Git 提交与打标]
```

1. **步骤 1：版本确定**
   - 在 `Cargo.toml` 中修改 `version` 为目标版本，并同步更新 `assets/mdreader.manifest`。

2. **步骤 2：本地构建与验证**
   - 执行：
     ```powershell
     Stop-Process -Name "MDReader" -Force -ErrorAction SilentlyContinue
     cargo test
     cargo build --release
     ```
   - 确保 **0 警告 0 错误**，所有单元与集成测试通过。

3. **步骤 3：自动化打包**
   - **打包策略区分**：
     - **正式发布版本（如 `v0.2.0` / `v1.0.0`）**：生成绿色便携目录 `Publish\MDReader_v<Version>\` 并归档压缩为 `Publish\MDReader-v<Version>-win-x64.zip`；
     - **内部调试版本（如调试流水号迭代）**：仅输出免装目录 `Publish\MDReader_Debug_v<Version>\`，不生成 ZIP 压缩包，便于本地快速调试验证。
   - **命令执行规范**：
     - **正式发布打包（绿色单二进制 + 配套脚本与配置模板 + ZIP 归档）**：
       ```powershell
       # 1. 检查并清理旧进程与历史产物
       Stop-Process -Name "MDReader" -Force -ErrorAction SilentlyContinue
       $ver = "<Version>"
       $publishBase = "Publish"
       $outDir = "$publishBase\MDReader_v$ver"
       $zipOut = "$publishBase\MDReader-v$ver-win-x64.zip"
       Remove-Item $outDir, $zipOut -Recurse -Force -ErrorAction SilentlyContinue
       New-Item -ItemType Directory -Force -Path "$outDir\scripts" | Out-Null

       # 2. 编译 Release 原生二进制（零外部运行时依赖）
       cargo build --release
       if ($LASTEXITCODE -ne 0) { throw "Release 构建失败" }

       # 3. 收集发行资产
       Copy-Item "target\release\MDReader.exe" "$outDir\MDReader.exe" -Force
       Copy-Item "config.example.json" "$outDir\config.example.json" -Force
       Copy-Item "README.md" "$outDir\README.md" -Force
       Copy-Item "scripts\register-md-assoc.ps1" "$outDir\scripts\register-md-assoc.ps1" -Force
       Copy-Item "scripts\unregister-md-assoc.ps1" "$outDir\scripts\unregister-md-assoc.ps1" -Force

       # 4. 自动归档压缩为标准 ZIP 发布包
       Compress-Archive -Path "$outDir\*" -DestinationPath $zipOut -Force
       ```
     - **内部调试打包（仅输出目录，不生成 ZIP）**：
       ```powershell
       Stop-Process -Name "MDReader" -Force -ErrorAction SilentlyContinue
       $ver = "<Version>"
       $outDir = "Publish\MDReader_Debug_v$ver"
       Remove-Item $outDir -Recurse -Force -ErrorAction SilentlyContinue
       New-Item -ItemType Directory -Force -Path "$outDir\scripts" | Out-Null
       cargo build --release
       if ($LASTEXITCODE -ne 0) { throw "Release 构建失败" }
       Copy-Item "target\release\MDReader.exe" "$outDir\MDReader.exe" -Force
       Copy-Item "config.example.json" "$outDir\config.example.json" -Force
       Copy-Item "scripts\register-md-assoc.ps1" "$outDir\scripts\register-md-assoc.ps1" -Force
       Copy-Item "scripts\unregister-md-assoc.ps1" "$outDir\scripts\unregister-md-assoc.ps1" -Force
       ```

4. **步骤 4：Git 提交与打标**
   - 暂存必要文件，禁止包含 `target/`、`Publish/`、`reports/` 和 `*.zip`。
   - 提交信息遵循 Conventional Commits：
     ```powershell
     git commit -m "chore(release): v<Version> - <变更摘要>"
     ```
   - 创建附注标签：
     ```powershell
     git tag -a v<Version> -m "Release v<Version>"
     ```

## 五、发布

自动化编译打包完成后，执行发布流程，共三步：

```text
[步骤 1: 文档与更新日志同步] -> [步骤 2: 推送远端分支与标签] -> [步骤 3: 发布]
```

1. **步骤 1：文档与更新日志同步**
   - 汇总自上一个正式版本以来的全部功能级变更，去除琐碎内部调试流水记录，保持更新日志清晰整洁。
   - 同步更新 `README.md` 中的特性说明、构建与配置用法。

2. **步骤 2：推送远端分支与标签**
   ```powershell
   git push origin main
   git push origin v<Version>
   ```

3. **步骤 3：发布**
   - 调用 GitHub CLI 发布：
     ```powershell
     gh release create v<Version> "Publish\MDReader-v<Version>-win-x64.zip" --title "MDReader v<Version>" --notes-file "<更新说明路径>"
     ```
   - 发布成功后，向用户汇报：
     - GitHub Release 链接
     - 打包资产名称及文件大小

## 六、智能体自动化触发契约

1. **发版自然语言意图响应**
   - 用户输入 **“打包”**、**“发布新版本”**、**“发版”**、**“发布 X.X”** 等意图时，智能体自动串联执行标准发布流水线。

2. **发布前确认门禁**
   - 执行远端推送与 GitHub Release 前，必须展示：
     - 目标版本号
     - 提交信息
     - 标签名称
     - 目标仓库
     - 打包资产路径
   - 等待用户一次明确确认后，方可执行 `git push` 与 `gh release create`。
   - “代码修改授权”不等于“远端发布授权”。

3. **失败处理**
   - 构建失败：停止发布，保留现场，输出错误日志。
   - 打包失败：清理半成品，不进入第五节。
   - 提交失败：检查 `git status`，不强行覆盖。
   - 推送失败：检查远端权限与网络，不强制推送。
   - Release 创建失败：保留已推送标签，输出手动补救命令。

4. **执行后闭环交付**
   - 智能体直接向用户汇报：
     - GitHub Release 链接
     - 打包资产名称及文件大小
     - 一句话指令，全自动闭环交付。
