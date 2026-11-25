#Requires -Version 5.1
<#
.SYNOPSIS
    MDReader 端到端黑盒测试套件 — Tier 3（跨功能两两组合测试，14 条）
.DESCRIPTION
    严格对齐 TEST_INFRA.md 中的 T3-01..T3-14 场景定义，覆盖共享状态、控制流与数据流联动。
    所有用例均为纯黑盒外部驱动，不依赖任何内部源码符号，并在 finally 中彻底清理进程、沙盒与 HKCU 注册表。
#>

Set-StrictMode -Version Latest

if (-not (Get-Command New-TestCaseDef -ErrorAction SilentlyContinue)) {
    Import-Module (Join-Path $PSScriptRoot '..\lib\E2EHarness.psm1') -Force -DisableNameChecking
}

$cases = New-Object 'System.Collections.Generic.List[object]'

# ==============================================================================
# T3-01: F2 + F3 — 注册 HKCU 关联后读取注册表命令模板并替换 %1 拉起含空格中文 .md 文件
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T3-01" -Tier 3 -Feature "F2+F3" -Group "G4" -Title "HKCU注册表模板替换%1拉起含空格中文.md文件" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T3-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId -CopyExeToSandbox
    $regSnap = Enter-HkcuRegistrySandbox
    $session = $null
    try {
        $docPath = Join-Path $ctx.SandboxDir "测试 关联 笔记 (1).md"
        Write-BytesUtf8 -Path $docPath -Text "# 注册表关联拉起测试`n`n正文：通过 shell\open\command 模板替换启动。" -Bom $false -Eol 'LF'

        # 1. 执行 --register 写入 HKCU 关联
        $regRes = Invoke-AppCli -ExePath $ctx.ExePath -Arguments @("--register") -TimeoutMs $CaseEnv.TimeoutMs -WorkingDirectory $ctx.SandboxDir
        Assert-Equal -Actual $regRes.TimedOut -Expected $false -Message "--register 不应超时"
        Assert-Equal -Actual $regRes.HadUnexpectedModal -Expected $false -Message "--register 不应弹出意外模态框"
        Assert-Equal -Actual $regRes.ExitCode -Expected 0 -Message "--register 应返回退出码 0 (StdErr=$($regRes.StdErr))"

        # 2. 直接从 HKCU 读取注册表命令模板与 ProgID 关联
        $cmdKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\Classes\MDReader.Document\shell\open\command", $false)
        Assert-True ($null -ne $cmdKey) "注册表缺少 HKCU\Software\Classes\MDReader.Document\shell\open\command"
        try {
            $cmdTemplate = [string]$cmdKey.GetValue("", "")
        } finally {
            $cmdKey.Dispose()
        }
        Assert-Match -Text $cmdTemplate -Pattern '"%1"' -Message "命令模板必须包含引号包裹的 %1 占位符"

        $progIdKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\Classes\.md\OpenWithProgids", $false)
        Assert-True ($null -ne $progIdKey) "注册表缺少 .md\OpenWithProgids"
        try {
            Assert-True ($progIdKey.GetValueNames() -contains "MDReader.Document") ".md\OpenWithProgids 未包含 MDReader.Document"
        } finally {
            $progIdKey.Dispose()
        }

        # 3. 解析注册表命令模板中的可执行文件路径，并按 %1 替换规则构造参数拉起窗口
        Assert-Match -Text $cmdTemplate -Pattern '^\s*"([^"]+)"\s+"%1"\s*$' -Message "命令模板应符合 <exe> %1 契约"
        $null = ($cmdTemplate -match '^\s*"([^"]+)"\s+"%1"\s*$')
        $parsedExe = $Matches[1]
        Assert-Equal -Actual ([System.IO.Path]::GetFullPath($parsedExe).ToLowerInvariant()) -Expected ([System.IO.Path]::GetFullPath($ctx.ExePath).ToLowerInvariant()) -Message "命令模板中的 exe 路径应与被测程序一致"

        $substitutedCmd = $cmdTemplate.Replace('"%1"', ('"' + $docPath + '"'))
        Assert-True ($substitutedCmd.Contains($docPath)) "替换后的命令行应包含目标中文空格路径"

        $launchCtx = @{
            CaseId     = $ctx.CaseId
            RunId      = $ctx.RunId
            SandboxDir = $ctx.SandboxDir
            ExePath    = $parsedExe
        }
        $session = Start-AppInstance -Context $launchCtx -Arguments @($docPath) -TimeoutMs $CaseEnv.TimeoutMs
        Assert-Match -Text $session.Title -Pattern '测试 关联 笔记 \(1\)\.md.*阅读模式.*MDReader' -Message "通过模板拉起后的窗口标题应显示目标文档与阅读模式"

        $modal = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 300
        Assert-True ($null -eq $modal) "正常拉起文档不应出现错误弹窗"

        Stop-AppInstance -Session $session
        $session = $null

        # 4. 执行 --unregister 验证精确卸载
        $unregRes = Invoke-AppCli -ExePath $ctx.ExePath -Arguments @("--unregister") -TimeoutMs $CaseEnv.TimeoutMs -WorkingDirectory $ctx.SandboxDir
        Assert-Equal -Actual $unregRes.ExitCode -Expected 0 -Message "--unregister 应返回退出码 0"
        $afterCmdKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\Classes\MDReader.Document", $false)
        if ($null -ne $afterCmdKey) {
            $afterCmdKey.Dispose()
            throw "执行 --unregister 后 HKCU\Software\Classes\MDReader.Document 仍残留"
        }
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Restore-HkcuRegistrySandbox -Snapshot $regSnap
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# T3-02: F3 + F8 + F10 — 带空格与中文路径直开 -> Ctrl+E 改写 -> Ctrl+S 保存自动回读
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T3-02" -Tier 3 -Feature "F3+F8+F10" -Group "G4" -Title "空格中文路径直开编辑保存自动回读全闭环" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T3-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $subDir = Join-Path $ctx.SandboxDir "目录 带空格 (测试)"
        New-Item -ItemType Directory -Force -Path $subDir | Out-Null
        $docPath = Join-Path $subDir "中文 笔记 (二).md"
        $initialText = "# 中文路径测试`n`n初始段落：验证直开与模式切换。"
        Write-BytesUtf8 -Path $docPath -Text $initialText -Bom $false -Eol 'LF'

        $session = Start-AppInstance -Context $ctx -Arguments @($docPath) -TimeoutMs $CaseEnv.TimeoutMs
        Assert-Match -Text $session.Title -Pattern '中文 笔记 \(二\)\.md.*阅读模式' -Message "初始应处于阅读模式"

        $shotBefore = Join-Path $ctx.SandboxDir "t302_before.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotBefore

        # Ctrl+E 切入修改模式
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "按 Ctrl+E 后未进入修改模式" -Condition {
            $session.MainWindow.Current.Name -match '修改模式'
        }

        $editor = Find-Control -Root $session.MainWindow -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT|RichEdit" -MinHeight 80 -TimeoutMs 5000
        Assert-True ($null -ne $editor) "修改模式下未找到编辑区控件"
        $loadedEditText = Get-EditorText -EditorElement $editor
        Assert-Match -Text $loadedEditText -Pattern '初始段落：验证直开与模式切换' -Message "编辑区未加载原中文内容"

        $updatedText = "# 中文路径测试`n`n已更新段落：空格中文路径原子写回成功✅。"
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $editor -Text $updatedText -ReplaceAll -PreserveUndo

        $null = Wait-Condition -TimeoutMs 4000 -ErrorMessage "修改内容后窗口标题未出现 * 脏标记" -Condition {
            $session.MainWindow.Current.Name -match '\*'
        }

        # Ctrl+S 保存并自动切回阅读模式
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$editor.Current.NativeWindowHandle) -Ctrl -Vk 0x53
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "Ctrl+S 后未自动切回阅读模式且清除 * 标记" -Condition {
            $t = $session.MainWindow.Current.Name
            ($t -match '阅读模式') -and ($t -notmatch '\*')
        }

        $meta = Read-FileBytesMeta -Path $docPath
        $normDisk = ($meta.Text -replace "`r`n", "`n").TrimEnd("`n")
        $normExpected = ($updatedText -replace "`r`n", "`n").TrimEnd("`n")
        Assert-Equal -Actual $normDisk -Expected $normExpected -Message "保存后磁盘内容应与修改内容一致"

        $shotAfter = Join-Path $ctx.SandboxDir "t302_after.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotAfter
        $diff = Compare-WindowCaptureDiff -PngPathA $shotBefore -PngPathB $shotAfter
        Assert-True ($diff -gt 0.0) "保存自动切回阅读态后页面渲染应发生更新 (diff=$diff)"

        # 再次按 Ctrl+E 验证编辑区加载的是最新保存文本
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "二次按 Ctrl+E 未进入修改模式" -Condition {
            $session.MainWindow.Current.Name -match '修改模式'
        }
        $editor2 = Find-Control -Root $session.MainWindow -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT|RichEdit" -MinHeight 80 -TimeoutMs 5000
        $reloadedText = ((Get-EditorText -EditorElement $editor2) -replace "`r`n", "`n") -replace "`r", "`n"
        Assert-Match -Text $reloadedText -Pattern '已更新段落：空格中文路径原子写回成功' -Message "二次进入编辑态应显示最新保存文本"
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# T3-03: F6 + F8 + F9 — 暗色主题下进入修改态编辑并校验阅读/编辑配色一致性及脏标记 *
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T3-03" -Tier 3 -Feature "F6+F8+F9" -Group "G4" -Title "dark暗色主题下阅读与编辑区配色一致性及脏标记" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T3-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $cfgPath = Join-Path $ctx.SandboxDir "dark_cfg.json"
        Write-BytesUtf8 -Path $cfgPath -Text '{"theme":"dark","fontSize":16,"editorFontSize":15}' -Bom $false -Eol 'LF'
        $docPath = Join-Path $ctx.SandboxDir "dark_edit.md"
        Write-BytesUtf8 -Path $docPath -Text "# 暗色主题联合验证`n`n用于验证阅读区与编辑区均保持低眩光深色背景。" -Bom $false -Eol 'LF'

        $session = Start-AppInstance -Context $ctx -Arguments @($docPath, "--config", $cfgPath) -TimeoutMs $CaseEnv.TimeoutMs
        $lumRead = Wait-Condition -TimeoutMs 5000 -ErrorMessage "暗色阅读模式亮度采样异常" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -ge 0 -and $l -lt 95) { return $l }
            return $null
        }
        Assert-True ($lumRead -lt 95) "dark 主题阅读态中心亮度应 < 95，实际=$lumRead"

        # 点击「编辑」按钮进入修改态
        $toggleBtn = Find-Control -Root $session.MainWindow -AutomationId "IDC_MODE_TOGGLE" -FallbackNames @("编辑") -ExcludeTitleBar -TimeoutMs 4000
        Assert-True ($null -ne $toggleBtn) "阅读态未找到编辑按钮"
        Invoke-Button -Element $toggleBtn -MainHwnd $session.Hwnd

        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "点击编辑按钮后未进入修改模式" -Condition {
            $session.MainWindow.Current.Name -match '修改模式'
        }

        $editor = Find-Control -Root $session.MainWindow -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT|RichEdit" -MinHeight 80 -TimeoutMs 5000
        Assert-True ($null -ne $editor) "未找到编辑框控件"

        # 验证暗色主题在修改态同样生效（编辑区不刺眼白屏）
        $lumEdit = Get-WindowLuminance -Hwnd $session.Hwnd
        Assert-True ($lumEdit -ge 0 -and $lumEdit -lt 105) "dark 主题下编辑区背景亮度应与阅读区一致保持深色 (<105)，实际=$lumEdit"

        # 在编辑区追加文本并校验 * 脏标记
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $editor -Text "`n追加暗色编辑内容" -PreserveUndo
        $null = Wait-Condition -TimeoutMs 4000 -ErrorMessage "暗色编辑后标题未出现 * 脏标记" -Condition {
            $t = $session.MainWindow.Current.Name
            ($t -match '修改模式') -and ($t -match '\*')
        }

        # 取消并放弃改动返回阅读态，再次核对暗色亮度保持
        $cancelBtn = Find-Control -Root $session.MainWindow -AutomationId "IDC_EDIT_CANCEL" -FallbackNames @("取消", "返回") -ExcludeTitleBar -TimeoutMs 3000
        Assert-True ($null -ne $cancelBtn) "修改态未找到取消按钮"
        Invoke-Button -Element $cancelBtn -MainHwnd $session.Hwnd

        $modal = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 4000
        Assert-True ($null -ne $modal) "有未保存修改点取消时应弹出确认框"
        Invoke-ModalBucket -Modal $modal -Bucket Discard

        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "放弃修改后未回到阅读模式" -Condition {
            $t = $session.MainWindow.Current.Name
            ($t -match '阅读模式') -and ($t -notmatch '\*')
        }
        $lumAfter = Get-WindowLuminance -Hwnd $session.Hwnd
        Assert-True ($lumAfter -ge 0 -and $lumAfter -lt 95) "返回阅读态后暗色亮度应保持 (<95)，实际=$lumAfter"
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# T3-04: F11 + F6 + F7 — 修改 config.json -> Ctrl+Shift+R 热重载 -> Ctrl+F 查找验证
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T3-04" -Tier 3 -Feature "F11+F6+F7" -Group "G4" -Title "热重载主题排版参数后Ctrl+F页内查找交互正常" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T3-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "sample-gfm-full.md") -Destination (Join-Path $ctx.SandboxDir "doc.md") -Force
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "test-image.png") -Destination (Join-Path $ctx.SandboxDir "test-image.png") -Force
        $docPath = Join-Path $ctx.SandboxDir "doc.md"
        $cfgPath = Join-Path $ctx.SandboxDir "reload_cfg.json"
        Write-BytesUtf8 -Path $cfgPath -Text '{"theme":"light","fontSize":14,"maxWidth":620,"pagePadding":28}' -Bom $false -Eol 'LF'

        $session = Start-AppInstance -Context $ctx -Arguments @($docPath, "--config", $cfgPath) -TimeoutMs $CaseEnv.TimeoutMs
        $lumLight = Wait-Condition -TimeoutMs 5000 -ErrorMessage "初始亮色亮度未达预期" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -gt 165) { return $l }
            return $null
        }
        $shotLight = Join-Path $ctx.SandboxDir "t304_light.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotLight

        # 外部修改 JSON 配置（切 dark + 增大 fontSize 与 maxWidth）
        Write-BytesUtf8 -Path $cfgPath -Text '{"theme":"dark","fontSize":22,"maxWidth":980,"pagePadding":56}' -Bom $false -Eol 'LF'
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Shift -Vk 0x52

        $lumDark = Wait-Condition -TimeoutMs 5000 -ErrorMessage "Ctrl+Shift+R 热重载后未切换为暗色主题" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -ge 0 -and $l -lt 95) { return $l }
            return $null
        }
        Assert-True ($lumLight - $lumDark -gt 80) "热重载前后明暗亮度差应明显 (>80), light=$lumLight, dark=$lumDark"

        $shotDark = Join-Path $ctx.SandboxDir "t304_dark.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotDark
        $diff = Compare-WindowCaptureDiff -PngPathA $shotLight -PngPathB $shotDark
        Assert-True ($diff -gt 0.2) "热重载后画面差异度应 > 0.2，实际=$diff"

        # 新排版下执行 Ctrl+F 页内查找
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x46
        $findBox = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_INPUT" -ClassRegex "Edit|EDIT" -MaxHeight 60 -TimeoutMs 4000
        Assert-True ($null -ne $findBox) "热重载后按 Ctrl+F 应唤起查找输入框"

        $findHwnd = [IntPtr]$findBox.Current.NativeWindowHandle
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $findBox -Text "Direct2D" -ReplaceAll -PreserveUndo
        Start-Sleep -Milliseconds 180
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $findHwnd -Vk 0x0D

        $statusEl = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_STATUS" -TimeoutMs 1000
        if ($null -ne $statusEl) {
            Assert-Match -Text $statusEl.Current.Name -Pattern '1/\d+' -Message "查找 Direct2D 应命中至少 1 处"
        }

        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $findHwnd -Vk 0x1B
        $modal = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 300
        Assert-True ($null -eq $modal) "热重载与查找组合流程不应触发异常弹窗"
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# T3-05: F5 + F7 + F10 — 渲染复杂 GFM 文档 -> 编辑追加新章节并保存 -> Ctrl+F 搜索新标题命中
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T3-05" -Tier 3 -Feature "F5+F7+F10" -Group "G4" -Title "GFM多元素渲染后编辑追加章节保存并Ctrl+F命中" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T3-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "sample-gfm-full.md") -Destination (Join-Path $ctx.SandboxDir "gfm.md") -Force
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "test-image.png") -Destination (Join-Path $ctx.SandboxDir "test-image.png") -Force
        $docPath = Join-Path $ctx.SandboxDir "gfm.md"

        $session = Start-AppInstance -Context $ctx -Arguments @($docPath) -TimeoutMs $CaseEnv.TimeoutMs
        Assert-Match -Text $session.Title -Pattern 'gfm\.md.*阅读模式' -Message "GFM 文档应正常进入阅读模式"

        # 切入修改态并追加新章节
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "未进入修改模式" -Condition {
            $session.MainWindow.Current.Name -match '修改模式'
        }
        $editor = Find-Control -Root $session.MainWindow -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT|RichEdit" -MinHeight 80 -TimeoutMs 5000
        $origContent = (Get-EditorText -EditorElement $editor) -replace "`r`n", "`n"
        $marker = "UNIQUE_T3_05_MARKER"
        $newContent = $origContent.TrimEnd() + "`n`n## 新增验收章节 $marker`n`n用于验证保存回读后页内查找实时索引更新。"
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $editor -Text $newContent -ReplaceAll -PreserveUndo

        $null = Wait-Condition -TimeoutMs 3000 -ErrorMessage "追加章节后未出现 * 脏标记" -Condition {
            $session.MainWindow.Current.Name -match '\*'
        }

        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$editor.Current.NativeWindowHandle) -Ctrl -Vk 0x53
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "Ctrl+S 后未自动切回阅读模式" -Condition {
            $t = $session.MainWindow.Current.Name
            ($t -match '阅读模式') -and ($t -notmatch '\*')
        }

        $diskMeta = Read-FileBytesMeta -Path $docPath
        Assert-True ($diskMeta.Text.Contains($marker)) "磁盘文件未写入新增章节标记"

        # 回到阅读模式后直接 Ctrl+F 搜索新增章节标记
        $shotBeforeFind = Join-Path $ctx.SandboxDir "t305_before_find.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotBeforeFind

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x46
        $findBox = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_INPUT" -ClassRegex "Edit|EDIT" -MaxHeight 60 -TimeoutMs 4000
        Assert-True ($null -ne $findBox) "未弹出页内查找输入框"
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $findBox -Text $marker -ReplaceAll -PreserveUndo
        Start-Sleep -Milliseconds 180

        $statusEl = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_STATUS" -TimeoutMs 1200
        if ($null -ne $statusEl) {
            Assert-Match -Text $statusEl.Current.Name -Pattern '1/1|1/\d+' -Message "新增章节标题应在保存后立即可被 Ctrl+F 命中"
        }
        $shotAfterFind = Join-Path $ctx.SandboxDir "t305_after_find.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotAfterFind
        $findDiff = Compare-WindowCaptureDiff -PngPathA $shotBeforeFind -PngPathB $shotAfterFind
        Assert-True ($findDiff -gt 0.0) "查找高亮渲染后界面应有可见变化"
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# T3-06: F11(损坏局部) + F3 + F10 — 含越界字段 + 合法 dark 的 config.json 打开并完成编辑保存闭环
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T3-06" -Tier 3 -Feature "F11+F3+F10" -Group "G4" -Title "局部越界配置回退同时合法dark生效且编辑保存闭环正常" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T3-06" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $cfgPath = Join-Path $ctx.SandboxDir "partial_bad.json"
        Write-BytesUtf8 -Path $cfgPath -Text '{"theme":"dark","fontSize":-10,"maxWidth":999999,"lineHeight":"invalid","pagePadding":0}' -Bom $false -Eol 'LF'
        $docPath = Join-Path $ctx.SandboxDir "partial_cfg_doc.md"
        Write-BytesUtf8 -Path $docPath -Text "# 局部配置容错测试`n`n初始内容。" -Bom $false -Eol 'LF'

        $session = Start-AppInstance -Context $ctx -Arguments @($docPath, "--config", $cfgPath) -TimeoutMs $CaseEnv.TimeoutMs
        $modal = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 400
        Assert-True ($null -eq $modal) "部分字段越界的 config.json 不应弹出阻断模态框"

        # 合法字段 theme="dark" 必须依然生效
        $lum = Wait-Condition -TimeoutMs 5000 -ErrorMessage "合法字段 theme=dark 未生效" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -ge 0 -and $l -lt 95) { return $l }
            return $null
        }
        Assert-True ($lum -lt 95) "暗色背景应生效，实际亮度=$lum"

        # 完成编辑保存闭环
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "未进入修改模式" -Condition {
            $session.MainWindow.Current.Name -match '修改模式'
        }
        $editor = Find-Control -Root $session.MainWindow -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT|RichEdit" -MinHeight 80 -TimeoutMs 5000
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $editor -Text "# 局部配置容错测试`n`n保存完成后的新内容。" -ReplaceAll -PreserveUndo
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$editor.Current.NativeWindowHandle) -Ctrl -Vk 0x53

        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "保存后未切回阅读模式" -Condition {
            $t = $session.MainWindow.Current.Name
            ($t -match '阅读模式') -and ($t -notmatch '\*')
        }

        $saved = Read-FileBytesMeta -Path $docPath
        Assert-Match -Text $saved.Text -Pattern '保存完成后的新内容' -Message "磁盘文件应包含新保存内容"
        $lumAfter = Get-WindowLuminance -Hwnd $session.Hwnd
        Assert-True ($lumAfter -lt 95) "保存切回阅读态后 dark 主题仍应保持"
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# T3-07: F8 + F9 + F4 — 编辑未保存触发脏标记 -> Ctrl+O 被确认框拦截 -> 选放弃并打开新文件且原文件无损
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T3-07" -Tier 3 -Feature "F8+F9+F4" -Group "G4" -Title "未保存修改按Ctrl+O拦截并选放弃切换新文件且原文件无损" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T3-07" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $docAPath = Join-Path $ctx.SandboxDir "docA.md"
        $docBPath = Join-Path $ctx.SandboxDir "docB.md"
        $origA = "# 文档 A`n`n原始无损内容 ALPHA。"
        $origB = "# 文档 B`n`n目标切换内容 BETA。"
        Write-BytesUtf8 -Path $docAPath -Text $origA -Bom $false -Eol 'LF'
        Write-BytesUtf8 -Path $docBPath -Text $origB -Bom $false -Eol 'LF'

        $session = Start-AppInstance -Context $ctx -Arguments @($docAPath) -TimeoutMs $CaseEnv.TimeoutMs
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "未进入修改模式" -Condition {
            $session.MainWindow.Current.Name -match '修改模式'
        }
        $editor = Find-Control -Root $session.MainWindow -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT|RichEdit" -MinHeight 80 -TimeoutMs 5000
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $editor -Text "# 文档 A`n`n未保存的脏修改 DIRTY_TEMP。" -ReplaceAll -PreserveUndo
        $null = Wait-Condition -TimeoutMs 3000 -ErrorMessage "未触发 * 脏标记" -Condition {
            $session.MainWindow.Current.Name -match '\*'
        }

        # 触发 Ctrl+O，先被未保存确认模态框拦截
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$editor.Current.NativeWindowHandle) -Ctrl -Vk 0x4F
        $confirmModal = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 5000
        Assert-True ($null -ne $confirmModal) "有未保存改动时触发 Ctrl+O 应弹出未保存确认框"
        $confirmHwnd = [IntPtr]$confirmModal.Current.NativeWindowHandle
        Invoke-ModalBucket -Modal $confirmModal -Bucket Discard

        # 确认放弃后，应弹出系统文件打开对话框 OpenFileDialog
        $openDlg = Wait-Condition -TimeoutMs 6000 -ErrorMessage "放弃未保存修改后未弹出文件打开对话框" -Condition {
            $d = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 0
            if ($null -ne $d -and ([IntPtr]$d.Current.NativeWindowHandle -ne $confirmHwnd)) {
                return $d
            }
            return $null
        }

        $fileEditCond = New-Object System.Windows.Automation.AndCondition(
            (New-Object System.Windows.Automation.PropertyCondition(
                [System.Windows.Automation.AutomationElement]::AutomationIdProperty, "1148"
            )),
            (New-Object System.Windows.Automation.PropertyCondition(
                [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
                [System.Windows.Automation.ControlType]::Edit
            ))
        )
        $fileBox = Wait-Condition -TimeoutMs 5000 -ErrorMessage "文件对话框中未找到文件名输入框" -Condition {
            $ed = $openDlg.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $fileEditCond)
            if ($null -ne $ed -and -not $ed.Current.IsOffscreen) { return $ed }
            return $null
        }

        $vp = $null
        $filled = $false
        try {
            if ($fileBox.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$vp)) {
                $vp.SetValue($docBPath)
                $filled = $true
            }
        } catch {}
        if (-not $filled) {
            Set-EditorText -MainHwnd ([IntPtr]$openDlg.Current.NativeWindowHandle) -EditorElement $fileBox -Text $docBPath -ReplaceAll -PreserveUndo
        }
        Start-Sleep -Milliseconds 120

        # 精确匹配文件对话框「打开」按钮（AutomationId="1" 且 ControlType=Button，避免误命中文件列表项）
        $openBtnCond = New-Object System.Windows.Automation.AndCondition(
            (New-Object System.Windows.Automation.PropertyCondition(
                [System.Windows.Automation.AutomationElement]::AutomationIdProperty, "1"
            )),
            (New-Object System.Windows.Automation.PropertyCondition(
                [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
                [System.Windows.Automation.ControlType]::Button
            ))
        )
        $openConfirmBtn = $openDlg.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $openBtnCond)
        if ($null -ne $openConfirmBtn) {
            Invoke-Button -Element $openConfirmBtn -MainHwnd ([IntPtr]$openDlg.Current.NativeWindowHandle)
        } else {
            Send-Hotkey -MainHwnd ([IntPtr]$openDlg.Current.NativeWindowHandle) -FocusHwnd ([IntPtr]$fileBox.Current.NativeWindowHandle) -Vk 0x0D
        }

        # 等待切换为 docB.md 阅读模式
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "选择新文件后未切换到 docB.md 阅读模式" -Condition {
            $t = $session.MainWindow.Current.Name
            ($t -match 'docB\.md') -and ($t -match '阅读模式') -and ($t -notmatch '\*')
        }

        # 校验原文件 docA.md 在磁盘上保持绝对无损
        $metaA = Read-FileBytesMeta -Path $docAPath
        Assert-Equal -Actual (($metaA.Text -replace "`r`n", "`n").TrimEnd("`n")) -Expected ($origA.TrimEnd("`n")) -Message "原文件 docA.md 不应被脏修改污染"

        # 切编辑态确认当前加载内容确为 docB.md
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "docB 未进入修改模式" -Condition {
            $session.MainWindow.Current.Name -match '修改模式'
        }
        $editorB = Find-Control -Root $session.MainWindow -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT|RichEdit" -MinHeight 80 -TimeoutMs 5000
        Assert-Match -Text (Get-EditorText -EditorElement $editorB) -Pattern '目标切换内容 BETA' -Message "当前文档内容应为 docB.md"
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# T3-08: F9 + F10(只读保护) + F8 — 只读文件 Ctrl+S 失败留守 -> 外部解除只读 -> 再次 Ctrl+S 成功并切回阅读态
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T3-08" -Tier 3 -Feature "F9+F10+F8" -Group "G4" -Title "只读文件保存失败留守修改态解除只读后再次保存成功回读" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T3-08" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $docPath = Join-Path $ctx.SandboxDir "readonly_recover.md"
        $origText = "# 只读恢复测试`n`n原始只读正文。"
        Write-BytesUtf8 -Path $docPath -Text $origText -Bom $false -Eol 'LF'
        $fi = Get-Item -LiteralPath $docPath
        $fi.IsReadOnly = $true

        $session = Start-AppInstance -Context $ctx -Arguments @($docPath) -TimeoutMs $CaseEnv.TimeoutMs
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "未进入修改模式" -Condition {
            $session.MainWindow.Current.Name -match '修改模式'
        }

        $editor = Find-Control -Root $session.MainWindow -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT|RichEdit" -MinHeight 80 -TimeoutMs 5000
        $newText = "# 只读恢复测试`n`n解除只读后成功写入的文本。"
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $editor -Text $newText -ReplaceAll -PreserveUndo
        $null = Wait-Condition -TimeoutMs 3000 -ErrorMessage "未产生脏标记 *" -Condition {
            $session.MainWindow.Current.Name -match '\*'
        }

        # 第一次 Ctrl+S：因文件只读，必须失败并弹出错误提示框
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$editor.Current.NativeWindowHandle) -Ctrl -Vk 0x53
        $errModal = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 5000
        Assert-True ($null -ne $errModal) "向只读文件按 Ctrl+S 应弹出保存失败提示框"
        Invoke-ModalBucket -Modal $errModal -Bucket Ok

        # 等待弹窗消失，并确认停留在修改模式、保留脏状态与编辑内容
        $null = Wait-Condition -TimeoutMs 4000 -ErrorMessage "关闭错误弹窗后未及时清理模态框" -Condition {
            $null -eq (Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 0)
        }
        Assert-Match -Text $session.MainWindow.Current.Name -Pattern '修改模式' -Message "保存失败后必须停留在修改模式"
        Assert-Match -Text $session.MainWindow.Current.Name -Pattern '\*' -Message "保存失败后必须保留 * 脏标记"
        Assert-Match -Text (Get-EditorText -EditorElement $editor) -Pattern '解除只读后成功写入的文本' -Message "保存失败后编辑区未保存内容不得丢失"

        $diskBeforeUnlock = Read-FileBytesMeta -Path $docPath
        Assert-Equal -Actual (($diskBeforeUnlock.Text -replace "`r`n", "`n").TrimEnd("`n")) -Expected ($origText.TrimEnd("`n")) -Message "只读期间磁盘原文件不得被篡改"

        # 外部解除只读属性
        $fi.Refresh()
        $fi.IsReadOnly = $false
        Assert-Equal -Actual $fi.IsReadOnly -Expected $false -Message "未能解除文件只读属性"

        # 第二次 Ctrl+S：应成功写回磁盘并自动切回阅读模式
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$editor.Current.NativeWindowHandle) -Ctrl -Vk 0x53
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "解除只读后再次 Ctrl+S 未自动切回阅读模式" -Condition {
            $t = $session.MainWindow.Current.Name
            ($t -match '阅读模式') -and ($t -notmatch '\*')
        }

        $diskAfterUnlock = Read-FileBytesMeta -Path $docPath
        Assert-Match -Text $diskAfterUnlock.Text -Pattern '解除只读后成功写入的文本' -Message "解除只读后再次保存应真实写入磁盘"
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# T3-09: F10(BOM+CRLF) + F3 + F5 — 带 UTF-8 BOM + CRLF 的复杂 GFM 文件编辑保存保留编码并正常渲染
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T3-09" -Tier 3 -Feature "F10+F3+F5" -Group "G4" -Title "UTF8-BOM与CRLF复杂GFM文档编辑保存保留字节编码与渲染" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T3-09" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "test-image.png") -Destination (Join-Path $ctx.SandboxDir "test-image.png") -Force
        $rawGfm = [System.IO.File]::ReadAllText((Join-Path $CaseEnv.FixturesDir "sample-gfm-full.md"), [System.Text.Encoding]::UTF8)
        $docPath = Join-Path $ctx.SandboxDir "gfm_bom_crlf.md"
        Write-BytesUtf8 -Path $docPath -Text $rawGfm -Bom $true -Eol 'CRLF'

        $preMeta = Read-FileBytesMeta -Path $docPath
        Assert-Equal -Actual $preMeta.HasUtf8Bom -Expected $true -Message "前置样本应包含 UTF-8 BOM"
        Assert-Equal -Actual $preMeta.LineEnding -Expected 'CRLF' -Message "前置样本应为纯 CRLF"

        $session = Start-AppInstance -Context $ctx -Arguments @($docPath) -TimeoutMs $CaseEnv.TimeoutMs
        Assert-Match -Text $session.Title -Pattern 'gfm_bom_crlf\.md.*阅读模式' -Message "BOM+CRLF 文档应正常进入阅读模式"

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "未进入修改模式" -Condition {
            $session.MainWindow.Current.Name -match '修改模式'
        }
        $editor = Find-Control -Root $session.MainWindow -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT|RichEdit" -MinHeight 80 -TimeoutMs 5000
        $curEdit = (Get-EditorText -EditorElement $editor) -replace "`r`n", "`n"
        $patched = $curEdit.TrimEnd() + "`n`n- [x] BOM与CRLF保留验证项`n| `core::bom` | 保留校验 | 0.1 MB | 通过 |"
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $editor -Text $patched -ReplaceAll -PreserveUndo

        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$editor.Current.NativeWindowHandle) -Ctrl -Vk 0x53
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "保存后未切回阅读模式" -Condition {
            $t = $session.MainWindow.Current.Name
            ($t -match '阅读模式') -and ($t -notmatch '\*')
        }

        $postMeta = Read-FileBytesMeta -Path $docPath
        Assert-Equal -Actual $postMeta.HasUtf8Bom -Expected $true -Message "保存后必须严格保留 UTF-8 BOM (EF BB BF)"
        Assert-Equal -Actual $postMeta.LineEnding -Expected 'CRLF' -Message "保存后必须严格保留纯 CRLF 换行风格"
        Assert-Match -Text $postMeta.Text -Pattern 'BOM与CRLF保留验证项' -Message "新追加列表项应写入磁盘"
        Assert-Match -Text $postMeta.Text -Pattern 'core::bom' -Message "新追加表格行应写入磁盘"

        $modal = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 300
        Assert-True ($null -eq $modal) "保存切回阅读后渲染表格/列表不应报错"
        $lum = Get-WindowLuminance -Hwnd $session.Hwnd
        Assert-True ($lum -gt 120) "阅读区应正常渲染（非黑屏/崩溃）"
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# T3-10: F3(空态) + F4 + F8 + F10 — 无参启动空态 -> 拖放打开新文件进入阅读态 -> 编辑保存闭环
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T3-10" -Tier 3 -Feature "F3+F4+F8+F10" -Group "G4" -Title "无参空态启动后拖放打开文件并完成编辑保存闭环" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T3-10" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $docPath = Join-Path $ctx.SandboxDir "拖放 新文件 (10).md"
        Write-BytesUtf8 -Path $docPath -Text "# 空态拖放打开`n`n初始内容。" -Bom $false -Eol 'LF'

        # 无参启动进入极简空态
        $session = Start-AppInstance -Context $ctx -Arguments @() -TimeoutMs $CaseEnv.TimeoutMs
        Assert-Match -Text $session.Title -Pattern '阅读模式.*MDReader' -Message "无参启动应进入极简阅读空态"

        # 确认在空态下误按 Ctrl+E 不崩溃
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        Start-Sleep -Milliseconds 150
        $modalEmpty = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 200
        Assert-True ($null -eq $modalEmpty) "空态下按 Ctrl+E 不应产生崩溃或错误模态框"

        # 通过 WM_DROPFILES 投递带空格中文文件
        Send-DropFiles -Hwnd $session.Hwnd -Paths @($docPath)
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "拖放文件后窗口标题未切换到目标文件" -Condition {
            $t = $session.MainWindow.Current.Name
            ($t -match '拖放 新文件 \(10\)\.md') -and ($t -match '阅读模式')
        }

        # 切入修改态编辑并保存回读
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "拖放打开文件后未能进入修改模式" -Condition {
            $session.MainWindow.Current.Name -match '修改模式'
        }
        $editor = Find-Control -Root $session.MainWindow -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT|RichEdit" -MinHeight 80 -TimeoutMs 5000
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $editor -Text "# 空态拖放打开`n`n拖放后编辑并保存成功。" -ReplaceAll -PreserveUndo
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$editor.Current.NativeWindowHandle) -Ctrl -Vk 0x53

        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "保存后未切回阅读模式" -Condition {
            $t = $session.MainWindow.Current.Name
            ($t -match '阅读模式') -and ($t -notmatch '\*')
        }
        $meta = Read-FileBytesMeta -Path $docPath
        Assert-Match -Text $meta.Text -Pattern '拖放后编辑并保存成功' -Message "磁盘应保存拖放后编辑的文本"
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# T3-11: F11(配置优先级) + F6 — 同时存在同级 config.json 与 --config 时，--config 优先级更高
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T3-11" -Tier 3 -Feature "F11+F6" -Group "G4" -Title "--config命令行配置优先级高于同级便携config.json" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T3-11" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId -CopyExeToSandbox
    $session = $null
    try {
        $docPath = Join-Path $ctx.SandboxDir "priority.md"
        Write-BytesUtf8 -Path $docPath -Text "# 配置优先级验证`n`n同级 light 与 --config dark 优先级对照。" -Bom $false -Eol 'LF'

        # 同级便携 config.json 设为 light
        $portableCfg = Join-Path $ctx.SandboxDir "config.json"
        Write-BytesUtf8 -Path $portableCfg -Text '{"theme":"light","fontSize":16}' -Bom $false -Eol 'LF'

        # --config 显式配置设为 dark
        $overrideDir = Join-Path $ctx.SandboxDir "override"
        New-Item -ItemType Directory -Force -Path $overrideDir | Out-Null
        $explicitCfg = Join-Path $overrideDir "custom.json"
        Write-BytesUtf8 -Path $explicitCfg -Text '{"theme":"dark","fontSize":18}' -Bom $false -Eol 'LF'

        # 1. 带 --config 启动：应以 dark 为准
        $session = Start-AppInstance -Context $ctx -Arguments @($docPath, "--config", $explicitCfg) -TimeoutMs $CaseEnv.TimeoutMs
        $lumOverride = Wait-Condition -TimeoutMs 5000 -ErrorMessage "--config dark 未覆盖同级 light 配置" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -ge 0 -and $l -lt 95) { return $l }
            return $null
        }
        Assert-True ($lumOverride -lt 95) "传入 --config 时应以暗色优先，实际亮度=$lumOverride"
        Stop-AppInstance -Session $session
        $session = $null

        # 2. 不带 --config 启动：应回落使用同级便携 config.json (light)
        $session = Start-AppInstance -Context $ctx -Arguments @($docPath) -TimeoutMs $CaseEnv.TimeoutMs
        $lumPortable = Wait-Condition -TimeoutMs 5000 -ErrorMessage "未传 --config 时同级便携 light 配置未生效" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -gt 165) { return $l }
            return $null
        }
        Assert-True ($lumPortable -gt 165) "不传 --config 时同级便携 light 应生效，实际亮度=$lumPortable"
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# T3-12: F5(XSS+图片) + F7(划词复制) — 含 <script> 与本地图片文档划词 Ctrl+C 复制且无脚本执行
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T3-12" -Tier 3 -Feature "F5+F7" -Group "G4" -Title "含XSS标签与本地图片文档鼠标划词复制且无脚本执行" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T3-12" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "sample-gfm-full.md") -Destination (Join-Path $ctx.SandboxDir "xss_img.md") -Force
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "test-image.png") -Destination (Join-Path $ctx.SandboxDir "test-image.png") -Force
        $docPath = Join-Path $ctx.SandboxDir "xss_img.md"

        $session = Start-AppInstance -Context $ctx -Arguments @($docPath) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 250

        # 确认无脚本 alert(1) 弹窗，且无任何浏览器/脚本子进程
        $modal = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 350
        Assert-True ($null -eq $modal) "含 <script>/onerror 的 Markdown 不得触发脚本弹窗"
        $metrics = Get-ProcessTreeMetrics -Process $session.Process
        Assert-Equal -Actual $metrics.ProcessCount -Expected 1 -Message "纯原生进程不应派生脚本/浏览器子进程"

        # 写入剪贴板哨兵，再跨居中正文区域划词拖拽并 Ctrl+C
        $sentinel = "CLIPBOARD_SENTINEL_T3_12"
        Set-ClipboardTextWin32 -Text $sentinel

        # 优先定位阅读画布控件 IDC_READER（无前景桌面回退时需直接向画布 HWND 投递鼠标拖拽消息）
        $readerEl = Find-Control -Root $session.MainWindow -AutomationId "IDC_READER" -TimeoutMs 1000
        $dragHwnd = if ($null -ne $readerEl -and [IntPtr]$readerEl.Current.NativeWindowHandle -ne [IntPtr]::Zero) {
            [IntPtr]$readerEl.Current.NativeWindowHandle
        } else {
            $session.Hwnd
        }

        $cr = New-Object E2ENative+RECT
        [void][E2ENative]::GetClientRect($dragHwnd, [ref]$cr)
        $cw = [math]::Max(($cr.Right - $cr.Left), 600)
        $ch = [math]::Max(($cr.Bottom - $cr.Top), 400)
        $sx = [int]($cw * 0.15)
        $sy = [int]($ch * 0.12)
        $ex = [int]($cw * 0.80)
        $ey = [int]($ch * 0.35)

        Send-MouseDragSelect -Hwnd $dragHwnd -StartX $sx -StartY $sy -EndX $ex -EndY $ey
        Start-Sleep -Milliseconds 120
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x43

        $copied = Wait-Condition -TimeoutMs 4000 -ErrorMessage "阅读区划词后 Ctrl+C 未更新剪贴板文本" -Condition {
            $clip = Get-ClipboardTextWin32
            if (-not [string]::IsNullOrWhiteSpace($clip) -and ($clip -ne $sentinel)) {
                return $clip
            }
            return $null
        }
        Assert-True ($copied.Length -gt 0) "剪贴板应包含复制的正文内容"
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# T3-13: F9(撤销重做) + F10 + F8 — 输入两段内容 -> Ctrl+Z 撤销一段 -> Ctrl+S 保存自动回读
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T3-13" -Tier 3 -Feature "F9+F10+F8" -Group "G4" -Title "编辑两段后Ctrl+Z撤销一段再Ctrl+S保存仅含未撤销段" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T3-13" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $docPath = Join-Path $ctx.SandboxDir "undo_commit.md"
        Write-BytesUtf8 -Path $docPath -Text "# 撤销验证基线`n`n基础段落。" -Bom $false -Eol 'LF'

        $session = Start-AppInstance -Context $ctx -Arguments @($docPath) -TimeoutMs $CaseEnv.TimeoutMs
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "未进入修改模式" -Condition {
            $session.MainWindow.Current.Name -match '修改模式'
        }
        $editor = Find-Control -Root $session.MainWindow -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT|RichEdit" -MinHeight 80 -TimeoutMs 5000
        $editHwnd = [IntPtr]$editor.Current.NativeWindowHandle

        # 定位末尾，输入第一段（通过 ReplaceAll+PreserveUndo 生成独立撤销边界）
        $baseText = ((Get-EditorText -EditorElement $editor) -replace "`r`n", "`n").TrimEnd()
        $seg1 = $baseText + "`n`n第一段保留内容_KEEP_SEG1"
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $editor -Text $seg1 -ReplaceAll -PreserveUndo
        $null = Wait-Condition -TimeoutMs 3000 -ErrorMessage "第一段文本未写入编辑框" -Condition {
            (Get-EditorText -EditorElement $editor) -match 'KEEP_SEG1'
        }

        # 通过移动光标 + 间隔断开 RichEdit 输入合并，然后输入第二段
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $editHwnd -Ctrl -Vk 0x23 -Extended
        Start-Sleep -Milliseconds 180
        $seg2 = $seg1 + "`n`n第二段待撤销内容_UNDO_SEG2"
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $editor -Text $seg2 -ReplaceAll -PreserveUndo
        $null = Wait-Condition -TimeoutMs 3000 -ErrorMessage "第二段文本未写入编辑框" -Condition {
            (Get-EditorText -EditorElement $editor) -match 'UNDO_SEG2'
        }

        # 按 Ctrl+Z 撤销第二段输入
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $editHwnd -Ctrl -Vk 0x5A
        $afterUndo = Wait-Condition -TimeoutMs 4000 -ErrorMessage "Ctrl+Z 未撤销第二段内容" -Condition {
            $cur = Get-EditorText -EditorElement $editor
            if (($cur -notmatch 'UNDO_SEG2') -and ($cur -match 'KEEP_SEG1')) { return $cur }
            return $null
        }
        Assert-Match -Text $afterUndo -Pattern 'KEEP_SEG1' -Message "撤销第二段后第一段必须完整保留"

        # 保存并自动切回阅读模式
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $editHwnd -Ctrl -Vk 0x53
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "保存后未切回阅读模式" -Condition {
            $t = $session.MainWindow.Current.Name
            ($t -match '阅读模式') -and ($t -notmatch '\*')
        }

        $diskMeta = Read-FileBytesMeta -Path $docPath
        Assert-Match -Text $diskMeta.Text -Pattern 'KEEP_SEG1' -Message "磁盘文件应保留未撤销段落"
        Assert-True (-not $diskMeta.Text.Contains('UNDO_SEG2')) "磁盘文件不应包含已被 Ctrl+Z 撤销的段落"
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# T3-14: F11(热重载) + F12(内存/句柄) — 连续修改 config.json 并热重载 + 模式切换循环校验稳定性
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T3-14" -Tier 3 -Feature "F11+F12" -Group "G4" -Title "连续热重载与模式切换循环验证配置生效及内存句柄稳定" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T3-14" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $cfgPath = Join-Path $ctx.SandboxDir "loop_cfg.json"
        Write-BytesUtf8 -Path $cfgPath -Text '{"theme":"light","fontSize":16,"maxWidth":760}' -Bom $false -Eol 'LF'
        $docPath = Join-Path $ctx.SandboxDir "loop_doc.md"
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "sample-basic.md") -Destination $docPath -Force

        $session = Start-AppInstance -Context $ctx -Arguments @($docPath, "--config", $cfgPath) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 350
        $mBase = Get-ProcessTreeMetrics -Process $session.Process

        # 6 轮配置热重载 + 阅读/修改切换循环
        for ($i = 1; $i -le 6; $i++) {
            $isDark = (($i % 2) -eq 1)
            $themeStr = if ($isDark) { "dark" } else { "light" }
            $fs = 15 + ($i % 4) * 2
            Write-BytesUtf8 -Path $cfgPath -Text ("{`"theme`":`"$themeStr`",`"fontSize`":$fs,`"maxWidth`":760}") -Bom $false -Eol 'LF'
            Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Shift -Vk 0x52

            if ($isDark) {
                $null = Wait-Condition -TimeoutMs 4000 -ErrorMessage "第 $i 轮热重载暗色未生效" -Condition {
                    $l = Get-WindowLuminance -Hwnd $session.Hwnd
                    if ($l -ge 0 -and $l -lt 95) { return $true }
                    return $false
                }
            } else {
                $null = Wait-Condition -TimeoutMs 4000 -ErrorMessage "第 $i 轮热重载亮色未生效" -Condition {
                    $l = Get-WindowLuminance -Hwnd $session.Hwnd
                    if ($l -gt 165) { return $true }
                    return $false
                }
            }

            Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
            $null = Wait-Condition -TimeoutMs 4000 -ErrorMessage "第 $i 轮未能进入修改模式" -Condition {
                $session.MainWindow.Current.Name -match '修改模式'
            }
            Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
            $null = Wait-Condition -TimeoutMs 4000 -ErrorMessage "第 $i 轮未能切回阅读模式" -Condition {
                $session.MainWindow.Current.Name -match '阅读模式'
            }
        }

        $mEnd = Get-ProcessTreeMetrics -Process $session.Process
        Assert-Equal -Actual $mEnd.ProcessCount -Expected 1 -Message "循环结束后必须维持单进程 (无子进程派生)"
        foreach ($cn in $mEnd.ChildNames) {
            Assert-True ($cn -notmatch 'msedgewebview2|node') "禁止出现高内存子进程: $cn"
        }

        $wsDelta = [math]::Round(($mEnd.WorkingSetMB - $mBase.WorkingSetMB), 2)
        $handleDelta = $mEnd.HandleCount - $mBase.HandleCount
        Assert-True ($wsDelta -lt 45) "6 轮热重载与切换后 WorkingSet 增长应可控 (<45MB)，实际增量=$wsDelta MB"
        Assert-True ($handleDelta -lt 350) "6 轮循环后句柄增长应可控 (<350)，实际增量=$handleDelta"

        if (-not $CaseEnv.IsSelfTest) {
            Assert-True ($mEnd.WorkingSetMB -le 60) "真实 exe 稳定后 WorkingSet 应 <= 60 MB，实际=$($mEnd.WorkingSetMB) MB"
        } else {
            Assert-True ($mEnd.WorkingSetMB -gt 0 -and $mEnd.WorkingSetMB -lt 120) "SelfTest 下 WorkingSet 指标采集链路应正常且 < 120 MB，实际=$($mEnd.WorkingSetMB) MB"
        }

        $btns = Get-ToolbarActionButtons -Root $session.MainWindow
        Assert-True ($btns.Count -le 4) "极简工具栏操作按钮应 <= 4，实际=$($btns.Count)"

        return @{
            Metrics = @{
                WorkingSetMB       = $mEnd.WorkingSetMB
                PrivateMemoryMB    = $mEnd.PrivateMemoryMB
                HandleCount        = $mEnd.HandleCount
                ProcessCount       = $mEnd.ProcessCount
                BaseWorkingSetMB   = $mBase.WorkingSetMB
                WorkingSetDeltaMB  = $wsDelta
                HandleDelta        = $handleDelta
            }
        }
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

return $cases.ToArray()
