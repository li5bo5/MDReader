#Requires -Version 5.1
<#
.SYNOPSIS
    MDReader 端到端黑盒测试套件 — Tier 4（真实场景端到端工作流，6 条）
.DESCRIPTION
    严格对齐 TEST_INFRA.md 中的 T4-01..T4-06 真实使用场景与验收指标：
    个人日常笔记、综合技术文档、200KB+ 长文档、JSON 重启与热重载、HKCU 关联闭环、Working Set 与极简体验量化验收。
#>

Set-StrictMode -Version Latest

if (-not (Get-Command New-TestCaseDef -ErrorAction SilentlyContinue)) {
    Import-Module (Join-Path $PSScriptRoot '..\lib\E2EHarness.psm1') -Force -DisableNameChecking
}

$cases = New-Object 'System.Collections.Generic.List[object]'

# ==============================================================================
# T4-01: 个人日常笔记全生命周期 (F3, F5, F8, F9, F10)
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T4-01" -Tier 4 -Feature "F3+F5+F8+F9+F10" -Group "G4" -Title "个人日常笔记全生命周期：阅读核对、勾选待办、编辑保存自动回读" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T4-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $notePath = Join-Path $ctx.SandboxDir "2026-11-24 个人日常笔记.md"
        $rawNote = [System.IO.File]::ReadAllText((Join-Path $CaseEnv.FixturesDir "sample-notes.md"), [System.Text.Encoding]::UTF8)
        Write-BytesUtf8 -Path $notePath -Text $rawNote -Bom $false -Eol 'CRLF'

        $session = Start-AppInstance -Context $ctx -Arguments @($notePath) -TimeoutMs $CaseEnv.TimeoutMs
        Assert-Match -Text $session.Title -Pattern '2026-11-24 个人日常笔记\.md - 阅读模式 - MDReader' -Message "打开个人笔记应进入阅读模式"

        $shotReadBefore = Join-Path $ctx.SandboxDir "t401_read_before.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotReadBefore

        # 按 Ctrl+E 切入修改模式
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "按 Ctrl+E 未进入修改模式" -Condition {
            $session.MainWindow.Current.Name -match '修改模式'
        }
        $editor = Find-Control -Root $session.MainWindow -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT|RichEdit" -MinHeight 80 -TimeoutMs 5000
        Assert-True ($null -ne $editor) "修改态未显示编辑框"

        $curText = (Get-EditorText -EditorElement $editor) -replace "`r`n", "`n"
        Assert-Match -Text $curText -Pattern '- \[ \] 阅读《深入理解 DirectWrite 排版》第三章' -Message "编辑区应加载原待办清单"

        # 勾选任务 [x] 并新增今日复盘条目
        $updatedText = $curText.Replace(
            '- [ ] 阅读《深入理解 DirectWrite 排版》第三章',
            '- [x] 阅读《深入理解 DirectWrite 排版》第三章'
        ).TrimEnd() + "`n`n## 今日复盘`n`n- [x] 已完成 MDReader 端到端阅读与编辑闭环验收 ✅"

        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $editor -Text $updatedText -ReplaceAll -PreserveUndo
        $null = Wait-Condition -TimeoutMs 3000 -ErrorMessage "修改待办后标题未出现 * 脏标记" -Condition {
            $session.MainWindow.Current.Name -match '\*'
        }

        # 点击「保存并阅读」按钮（或 Ctrl+S）写回磁盘并自动切回阅读模式
        $saveBtn = Find-Control -Root $session.MainWindow -AutomationId "IDC_MODE_TOGGLE" -FallbackNames @("保存并阅读") -ExcludeTitleBar -TimeoutMs 3000
        Assert-True ($null -ne $saveBtn) "修改态未找到「保存并阅读」按钮"
        Invoke-Button -Element $saveBtn -MainHwnd $session.Hwnd

        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "保存后未自动切回阅读模式且清除 *" -Condition {
            $t = $session.MainWindow.Current.Name
            ($t -match '^2026-11-24 个人日常笔记\.md - 阅读模式 - MDReader$') -and ($t -notmatch '\*')
        }

        # 外部字节级校验磁盘文件与换行符保持
        $diskMeta = Read-FileBytesMeta -Path $notePath
        Assert-Equal -Actual $diskMeta.LineEnding -Expected 'CRLF' -Message "日常笔记原有 CRLF 换行风格应保持"
        Assert-Match -Text $diskMeta.Text -Pattern '- \[x\] 阅读《深入理解 DirectWrite 排版》第三章' -Message "磁盘应保存已勾选的待办项"
        Assert-Match -Text $diskMeta.Text -Pattern '已完成 MDReader 端到端阅读与编辑闭环验收' -Message "磁盘应包含新增的复盘段落"

        $shotReadAfter = Join-Path $ctx.SandboxDir "t401_read_after.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotReadAfter
        $diff = Compare-WindowCaptureDiff -PngPathA $shotReadBefore -PngPathB $shotReadAfter
        Assert-True ($diff -gt 0.0) "保存自动切回阅读模式后界面应渲染更新内容"

        # 再次切入编辑后无修改按 Esc，应无弹窗秒切回阅读态
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "二次进入修改模式失败" -Condition {
            $session.MainWindow.Current.Name -match '修改模式'
        }
        $editor2 = Find-Control -Root $session.MainWindow -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT|RichEdit" -MinHeight 80 -TimeoutMs 5000
        Assert-Match -Text (Get-EditorText -EditorElement $editor2) -Pattern '已完成 MDReader 端到端阅读与编辑闭环验收' -Message "再次编辑应读取最新保存的全文"

        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$editor2.Current.NativeWindowHandle) -Vk 0x1B
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "无修改按 Esc 未直接返回阅读模式" -Condition {
            $session.MainWindow.Current.Name -match '阅读模式'
        }
        $unexpectedModal = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 250
        Assert-True ($null -eq $unexpectedModal) "无修改返回阅读模式不应弹出确认框"
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# T4-02: 技术规格文档阅读与修订 (F5, F6, F7, F10)
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T4-02" -Tier 4 -Feature "F5+F6+F7+F10" -Group "G4" -Title "综合技术规格文档阅读、明暗切换、查找代码块与修订回读" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T4-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $docPath = Join-Path $ctx.SandboxDir "tech_spec.md"
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "sample-gfm-full.md") -Destination $docPath -Force
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "test-image.png") -Destination (Join-Path $ctx.SandboxDir "test-image.png") -Force
        $cfgPath = Join-Path $ctx.SandboxDir "spec_cfg.json"
        Write-BytesUtf8 -Path $cfgPath -Text '{"theme":"light","fontSize":16,"maxWidth":760,"pagePadding":40}' -Bom $false -Eol 'LF'

        $session = Start-AppInstance -Context $ctx -Arguments @($docPath, "--config", $cfgPath) -TimeoutMs $CaseEnv.TimeoutMs
        $lumLight = Wait-Condition -TimeoutMs 5000 -ErrorMessage "亮色主题亮度未达预期" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -gt 165) { return $l }
            return $null
        }
        $shotLight = Join-Path $ctx.SandboxDir "t402_light.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotLight

        # 切换暗色配置并热重载
        Write-BytesUtf8 -Path $cfgPath -Text '{"theme":"dark","fontSize":16,"maxWidth":760,"pagePadding":40}' -Bom $false -Eol 'LF'
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Shift -Vk 0x52
        $lumDark = Wait-Condition -TimeoutMs 5000 -ErrorMessage "热重载切暗色主题失败" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -ge 0 -and $l -lt 95) { return $l }
            return $null
        }
        $shotDark = Join-Path $ctx.SandboxDir "t402_dark.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotDark
        $themeDiff = Compare-WindowCaptureDiff -PngPathA $shotLight -PngPathB $shotDark
        Assert-True ($themeDiff -gt 0.25) "明暗主题切换后画面应有显著变化 (diff=$themeDiff)"

        # Ctrl+F 定位 Rust 代码块内容
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x46
        $findBox = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_INPUT" -ClassRegex "Edit|EDIT" -MaxHeight 60 -TimeoutMs 4000
        Assert-True ($null -ne $findBox) "按 Ctrl+F 应打开查找框"
        $findHwnd = [IntPtr]$findBox.Current.NativeWindowHandle
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $findBox -Text "MDReader 纯粹阅读器" -ReplaceAll -PreserveUndo
        Start-Sleep -Milliseconds 180
        $statusEl = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_STATUS" -TimeoutMs 1000
        if ($null -ne $statusEl) {
            Assert-Match -Text $statusEl.Current.Name -Pattern '1/\d+' -Message "应命中代码块中的关键字"
        }
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $findHwnd -Vk 0x1B

        # 切入修改模式修订 GFM 表格与代码块并 Ctrl+S 保存
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "未能进入修改模式" -Condition {
            $session.MainWindow.Current.Name -match '修改模式'
        }
        $editor = Find-Control -Root $session.MainWindow -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT|RichEdit" -MinHeight 80 -TimeoutMs 5000
        $rawEdit = (Get-EditorText -EditorElement $editor) -replace "`r`n", "`n"
        $patched = $rawEdit.Replace("测试中", "已验收").Replace('println!("启动成功: {}", title);', 'println!("启动成功: {}", title); // SPEC_VERIFIED_T402')
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $editor -Text $patched -ReplaceAll -PreserveUndo

        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$editor.Current.NativeWindowHandle) -Ctrl -Vk 0x53
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "修订保存后未自动切回阅读模式" -Condition {
            $t = $session.MainWindow.Current.Name
            ($t -match '阅读模式') -and ($t -notmatch '\*')
        }

        $diskMeta = Read-FileBytesMeta -Path $docPath
        Assert-Match -Text $diskMeta.Text -Pattern '已验收' -Message "GFM 表格状态修改应已落盘"
        Assert-Match -Text $diskMeta.Text -Pattern 'SPEC_VERIFIED_T402' -Message "代码块注释修订应已落盘"

        # 切回阅读态后再次 Ctrl+F 验证新修订标识符命中
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x46
        $findBox2 = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_INPUT" -ClassRegex "Edit|EDIT" -MaxHeight 60 -TimeoutMs 4000
        Assert-True ($null -ne $findBox2) "二次 Ctrl+F 未找到查找框"
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $findBox2 -Text "SPEC_VERIFIED_T402" -ReplaceAll -PreserveUndo
        Start-Sleep -Milliseconds 180
        $statusEl2 = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_STATUS" -TimeoutMs 1000
        if ($null -ne $statusEl2) {
            Assert-Match -Text $statusEl2.Current.Name -Pattern '1/\d+' -Message "修订后的标识符应在阅读模式查找中命中"
        }
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# T4-03: 长篇文档滚动、查找与局部微调 (F5, F7, F8, F9, F10)
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T4-03" -Tier 4 -Feature "F5+F7+F8+F9+F10" -Group "G4" -Title "200KB+长文档翻页滚动、查找尾部锚点与局部微调闭环" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T4-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $longSrc = Join-Path $CaseEnv.FixturesDir "sample-long.md"
        $longInfo = Get-Item -LiteralPath $longSrc
        Assert-True ($longInfo.Length -gt 200000) "sample-long.md 必须大于 200KB，实际=$($longInfo.Length)"

        $docPath = Join-Path $ctx.SandboxDir "long_doc.md"
        Copy-Item -LiteralPath $longSrc -Destination $docPath -Force

        $session = Start-AppInstance -Context $ctx -Arguments @($docPath) -TimeoutMs $CaseEnv.TimeoutMs
        Assert-Match -Text $session.Title -Pattern 'long_doc\.md.*阅读模式' -Message "长文档应正常进入阅读模式"

        $shotTop = Join-Path $ctx.SandboxDir "t403_top.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotTop

        # 模拟翻页滚动：PageDown x3 -> End -> Home
        for ($i = 0; $i -lt 3; $i++) {
            Send-Hotkey -MainHwnd $session.Hwnd -Vk 0x22 -Extended
            Start-Sleep -Milliseconds 120
        }
        $shotMid = Join-Path $ctx.SandboxDir "t403_mid.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotMid
        $scrollDiff = Compare-WindowCaptureDiff -PngPathA $shotTop -PngPathB $shotMid
        Assert-True ($scrollDiff -gt 0.0) "PageDown 翻页后阅读视口画面应发生变化 (diff=$scrollDiff)"

        Send-Hotkey -MainHwnd $session.Hwnd -Vk 0x23 -Extended
        Start-Sleep -Milliseconds 150
        Send-Hotkey -MainHwnd $session.Hwnd -Vk 0x24 -Extended
        Start-Sleep -Milliseconds 150

        # Ctrl+F 查找尾部锚点 TAIL_ANCHOR_MARKER_END
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x46
        $findBox = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_INPUT" -ClassRegex "Edit|EDIT" -MaxHeight 60 -TimeoutMs 4000
        Assert-True ($null -ne $findBox) "长文档阅读态按 Ctrl+F 应打开查找框"
        $findHwnd = [IntPtr]$findBox.Current.NativeWindowHandle
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $findBox -Text "TAIL_ANCHOR_MARKER_END" -ReplaceAll -PreserveUndo
        Start-Sleep -Milliseconds 220
        $statusEl = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_STATUS" -TimeoutMs 1200
        if ($null -ne $statusEl) {
            Assert-Match -Text $statusEl.Current.Name -Pattern '1/\d+' -Message "应在 200KB+ 文档尾部命中 TAIL_ANCHOR_MARKER_END"
        }
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $findHwnd -Vk 0x1B

        # 切修改模式并在尾部锚点附近追加修订词
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "长文档未能进入修改模式" -Condition {
            $session.MainWindow.Current.Name -match '修改模式'
        }
        $editor = Find-Control -Root $session.MainWindow -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT|RichEdit" -MinHeight 80 -TimeoutMs 5000
        $editHwnd = [IntPtr]$editor.Current.NativeWindowHandle

        # 跳至末尾追加新词（避免对 200KB 全文做 SendInput 逐字输入造成超时）
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $editHwnd -Ctrl -Vk 0x23 -Extended
        Start-Sleep -Milliseconds 120
        $patchWord = "LONG_DOC_PATCH_OK"
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $editor -Text "`n`n补充微调：$patchWord" -PreserveUndo

        $null = Wait-Condition -TimeoutMs 4000 -ErrorMessage "长文档末尾追加后标题未出现 * 脏标记" -Condition {
            $session.MainWindow.Current.Name -match '\*'
        }

        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $editHwnd -Ctrl -Vk 0x53
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "长文档保存后未自动切回阅读模式" -Condition {
            $t = $session.MainWindow.Current.Name
            ($t -match '阅读模式') -and ($t -notmatch '\*')
        }

        $diskMeta = Read-FileBytesMeta -Path $docPath
        Assert-True ($diskMeta.Length -gt 200000) "保存后文档大小应仍 > 200KB"
        Assert-True ($diskMeta.Text.Contains($patchWord)) "磁盘长文档应包含末尾微调内容 $patchWord"

        # 回到阅读态再次 Ctrl+F 查找新词
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x46
        $findBox2 = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_INPUT" -ClassRegex "Edit|EDIT" -MaxHeight 60 -TimeoutMs 4000
        Assert-True ($null -ne $findBox2) "二次查找未打开查找框"
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $findBox2 -Text $patchWord -ReplaceAll -PreserveUndo
        Start-Sleep -Milliseconds 200
        $statusEl2 = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_STATUS" -TimeoutMs 1200
        if ($null -ne $statusEl2) {
            Assert-Match -Text $statusEl2.Current.Name -Pattern '1/\d+' -Message "长文档局部微调保存后应可立即查找命中新词"
        }
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# T4-04: 个性化 JSON 排版工作流（重启 + 热重载）(F6, F11)
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T4-04" -Tier 4 -Feature "F6+F11" -Group "G4" -Title "零设置UI审计及JSON排版参数重启持久化与热重载生效" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T4-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $docPath = Join-Path $ctx.SandboxDir "layout.md"
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "sample-basic.md") -Destination $docPath -Force
        $cfgPath = Join-Path $ctx.SandboxDir "personal.json"
        Write-BytesUtf8 -Path $cfgPath -Text '{"theme":"light","fontSize":15,"maxWidth":700,"pagePadding":32}' -Bom $false -Eol 'LF'

        $session = Start-AppInstance -Context $ctx -Arguments @($docPath, "--config", $cfgPath) -TimeoutMs $CaseEnv.TimeoutMs

        # 1. 极简无设置界面核查
        $btns = Get-ToolbarActionButtons -Root $session.MainWindow
        Assert-True ($btns.Count -le 4) "主界面操作按钮应 <= 4，实际=$($btns.Count)"
        $allEls = $session.MainWindow.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
        foreach ($el in $allEls) {
            if ($el.Current.IsOffscreen) { continue }
            $n = [string]$el.Current.Name
            $aid = [string]$el.Current.AutomationId
            Assert-True ($n -notmatch '^(设置|首选项|Preferences|Settings|Options)$') "界面不得存在设置按钮或面板: Name='$n'"
            Assert-True ($aid -notmatch 'Settings|Preferences') "界面不得存在设置控件 ID: Aid='$aid'"
        }

        $lum1 = Wait-Condition -TimeoutMs 5000 -ErrorMessage "初始亮色排版亮度未达标" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -gt 165) { return $l }
            return $null
        }
        $shot1 = Join-Path $ctx.SandboxDir "t404_step1.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shot1

        Stop-AppInstance -Session $session
        $session = $null

        # 2. 外部改写 config.json 并重启验证持久化生效
        Write-BytesUtf8 -Path $cfgPath -Text '{"theme":"dark","fontSize":20,"maxWidth":960,"pagePadding":56,"colors":{"background":"#181A1F","text":"#E6EDF3"}}' -Bom $false -Eol 'LF'
        $session = Start-AppInstance -Context $ctx -Arguments @($docPath, "--config", $cfgPath) -TimeoutMs $CaseEnv.TimeoutMs

        $lum2 = Wait-Condition -TimeoutMs 5000 -ErrorMessage "重启后暗色自定义背景未生效" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -ge 0 -and $l -lt 70) { return $l }
            return $null
        }
        $shot2 = Join-Path $ctx.SandboxDir "t404_step2.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shot2
        $diff12 = Compare-WindowCaptureDiff -PngPathA $shot1 -PngPathB $shot2
        Assert-True ($diff12 -gt 0.25) "修改 JSON 重启后排版与配色应明显生效 (diff=$diff12)"

        # 3. 运行期再次修改 JSON 并通过 Ctrl+Shift+R 热重载生效
        Write-BytesUtf8 -Path $cfgPath -Text '{"theme":"light","fontSize":18,"maxWidth":820,"pagePadding":44,"colors":{"background":"#FFFDF7","text":"#1A1A1A"}}' -Bom $false -Eol 'LF'
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Shift -Vk 0x52

        $lum3 = Wait-Condition -TimeoutMs 5000 -ErrorMessage "Ctrl+Shift+R 热重载自定义亮色纸张底色未生效" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -gt 180) { return $l }
            return $null
        }
        Assert-True ($lum3 -gt 180 -and $lum1 -gt 165 -and $lum2 -lt 70) "三阶段明暗亮度应符合预期 (lum1=$lum1, lum2=$lum2, lum3=$lum3)"
        $shot3 = Join-Path $ctx.SandboxDir "t404_step3.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shot3
        $diff23 = Compare-WindowCaptureDiff -PngPathA $shot2 -PngPathB $shot3
        Assert-True ($diff23 -gt 0.25) "热重载后界面渲染应明显更新 (diff=$diff23)"
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# T4-05: Windows 注册集成拉起与编辑闭环 (F2, F3, F8, F10)
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T4-05" -Tier 4 -Feature "F2+F3+F8+F10" -Group "G4" -Title "沙盒注册--register解析命令拉起编辑保存并--unregister零残留" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T4-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId -CopyExeToSandbox
    $regSnap = Enter-HkcuRegistrySandbox
    $session = $null
    try {
        $notePath = Join-Path $ctx.SandboxDir "个人 文档 (关联闭环).md"
        Write-BytesUtf8 -Path $notePath -Text "# Windows 关联拉起闭环`n`n原始文档正文。" -Bom $true -Eol 'CRLF'

        # 1. 注册 HKCU 关联
        $regRes = Invoke-AppCli -ExePath $ctx.ExePath -Arguments @("--register") -TimeoutMs $CaseEnv.TimeoutMs -WorkingDirectory $ctx.SandboxDir
        Assert-Equal -Actual $regRes.ExitCode -Expected 0 -Message "--register 执行应成功"
        Assert-Equal -Actual $regRes.HadUnexpectedModal -Expected $false -Message "--register 不应弹窗"

        # 2. 核验 HKCU 完整注册结构并读取 shell\open\command
        $cmdKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\Classes\MDReader.Document\shell\open\command", $false)
        Assert-True ($null -ne $cmdKey) "缺少 MDReader.Document\shell\open\command"
        try {
            $cmdTemplate = [string]$cmdKey.GetValue("", "")
        } finally {
            $cmdKey.Dispose()
        }
        Assert-Match -Text $cmdTemplate -Pattern '^\s*"([^"]+)"\s+"%1"\s*$' -Message "命令模板格式应为 <exe> %1"
        $null = ($cmdTemplate -match '^\s*"([^"]+)"\s+"%1"\s*$')
        $parsedExe = $Matches[1]
        Assert-Equal -Actual ([System.IO.Path]::GetFullPath($parsedExe).ToLowerInvariant()) -Expected ([System.IO.Path]::GetFullPath($ctx.ExePath).ToLowerInvariant()) -Message "注册表中可执行文件路径应指向沙盒内被测 exe"

        $capKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\MDReader\Capabilities\FileAssociations", $false)
        Assert-True ($null -ne $capKey) "缺少 Software\MDReader\Capabilities\FileAssociations"
        try {
            Assert-Equal -Actual ([string]$capKey.GetValue(".md", "")) -Expected "MDReader.Document" -Message "Capabilities\.md 应指向 MDReader.Document"
        } finally {
            $capKey.Dispose()
        }

        # 3. 依据模板解析的 exe 与 %1 替换路径启动窗口并完成编辑保存闭环
        $launchCtx = @{
            CaseId     = $ctx.CaseId
            RunId      = $ctx.RunId
            SandboxDir = $ctx.SandboxDir
            ExePath    = $parsedExe
        }
        $session = Start-AppInstance -Context $launchCtx -Arguments @($notePath) -TimeoutMs $CaseEnv.TimeoutMs
        Assert-Match -Text $session.Title -Pattern '个人 文档 \(关联闭环\)\.md.*阅读模式' -Message "通过注册表命令拉起应进入阅读模式"

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "未进入修改模式" -Condition {
            $session.MainWindow.Current.Name -match '修改模式'
        }
        $editor = Find-Control -Root $session.MainWindow -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT|RichEdit" -MinHeight 80 -TimeoutMs 5000
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $editor -Text "# Windows 关联拉起闭环`n`n关联拉起后已编辑保存回读成功。" -ReplaceAll -PreserveUndo

        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$editor.Current.NativeWindowHandle) -Ctrl -Vk 0x53
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "保存后未自动切回阅读模式" -Condition {
            $t = $session.MainWindow.Current.Name
            ($t -match '阅读模式') -and ($t -notmatch '\*')
        }
        Stop-AppInstance -Session $session
        $session = $null

        $savedMeta = Read-FileBytesMeta -Path $notePath
        Assert-Equal -Actual $savedMeta.HasUtf8Bom -Expected $true -Message "关联拉起保存后应保留 UTF-8 BOM"
        Assert-Equal -Actual $savedMeta.LineEnding -Expected 'CRLF' -Message "关联拉起保存后应保留 CRLF"
        Assert-Match -Text $savedMeta.Text -Pattern '关联拉起后已编辑保存回读成功' -Message "磁盘文件应包含修改后内容"

        # 4. 运行 --unregister 并核验零残留
        $unregRes = Invoke-AppCli -ExePath $ctx.ExePath -Arguments @("--unregister") -TimeoutMs $CaseEnv.TimeoutMs -WorkingDirectory $ctx.SandboxDir
        Assert-Equal -Actual $unregRes.ExitCode -Expected 0 -Message "--unregister 应返回退出码 0"

        foreach ($gonePath in @("Software\Classes\MDReader.Document", "Software\MDReader")) {
            $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($gonePath, $false)
            if ($null -ne $k) {
                $k.Dispose()
                throw "卸载后仍残留注册表键: HKCU\$gonePath"
            }
        }
        foreach ($extPath in @("Software\Classes\.md\OpenWithProgids", "Software\Classes\.markdown\OpenWithProgids")) {
            $ek = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($extPath, $false)
            if ($null -ne $ek) {
                try {
                    Assert-True ($ek.GetValueNames() -notcontains "MDReader.Document") "卸载后 $extPath 不应残留 MDReader.Document"
                } finally {
                    $ek.Dispose()
                }
            }
        }
        $raKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\RegisteredApplications", $false)
        if ($null -ne $raKey) {
            try {
                Assert-True ($raKey.GetValueNames() -notcontains "MDReader") "卸载后 RegisteredApplications 不应残留 MDReader"
            } finally {
                $raKey.Dispose()
            }
        }
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Restore-HkcuRegistrySandbox -Snapshot $regSnap
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# T4-06: Working Set 内存基线与极简体验量化验收 (F11, F12)
# ==============================================================================
[void]$cases.Add((New-TestCaseDef -Id "T4-06" -Tier 4 -Feature "F11+F12" -Group "G4" -Title "冷启动与100KB文档WorkingSet内存基线、句柄进程数及极简体验量化验收" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T4-06" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        # 1. 空载冷启动采样与极简体验结构量化
        $session = Start-AppInstance -Context $ctx -Arguments @() -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 500
        $mCold = Get-ProcessTreeMetrics -Process $session.Process

        $toolbarBtns = Get-ToolbarActionButtons -Root $session.MainWindow
        $btnCount = $toolbarBtns.Count
        Assert-True ($btnCount -ge 1 -and $btnCount -le 4) "极简主窗口操作按钮数必须 <= 4，实际=$btnCount"

        # 核查无设置面板/偏好项
        $allEls = $session.MainWindow.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
        foreach ($el in $allEls) {
            if ($el.Current.IsOffscreen) { continue }
            $n = [string]$el.Current.Name
            Assert-True ($n -notmatch '^(设置|首选项|Preferences|Settings|Options)$') "极简界面不得包含设置面板: $n"
        }

        # 计算阅读正文视口占比（>= 85%）
        $cr = New-Object E2ENative+RECT
        [void][E2ENative]::GetClientRect($session.Hwnd, [ref]$cr)
        $clientH = [math]::Max(($cr.Bottom - $cr.Top), 1)
        $pt = New-Object E2ENative+POINT
        $pt.X = 0; $pt.Y = 0
        [void][E2ENative]::ClientToScreen($session.Hwnd, [ref]$pt)
        $maxBtnBottom = 0
        foreach ($b in $toolbarBtns) {
            $br = $b.Current.BoundingRectangle
            $relB = [int]($br.Bottom - $pt.Y)
            if ($relB -gt $maxBtnBottom) { $maxBtnBottom = $relB }
        }
        $toolbarH = [math]::Max(($maxBtnBottom + 6), 36)
        $viewportRatio = [math]::Round((($clientH - $toolbarH) / [double]$clientH), 3)
        Assert-True ($viewportRatio -ge 0.85) "阅读区视口面积占比应 >= 0.85，实际=$viewportRatio (clientH=$clientH, toolbarH=$toolbarH)"

        Stop-AppInstance -Session $session
        $session = $null

        # 2. 构造 ~100KB 典型 Markdown 文档并采样打开后内存
        $longText = [System.IO.File]::ReadAllText((Join-Path $CaseEnv.FixturesDir "sample-long.md"), [System.Text.Encoding]::UTF8)
        $doc100kPath = Join-Path $ctx.SandboxDir "typical_100kb.md"
        $targetChars = [math]::Min($longText.Length, 42000)
        $slice100k = $longText.Substring(0, $targetChars)
        while ([System.Text.Encoding]::UTF8.GetByteCount($slice100k) -lt 100000 -and $slice100k.Length -lt $longText.Length) {
            $nextLen = [math]::Min($longText.Length, $slice100k.Length + 4000)
            $slice100k = $longText.Substring(0, $nextLen)
        }
        Write-BytesUtf8 -Path $doc100kPath -Text $slice100k -Bom $false -Eol 'LF'
        $doc100kBytes = (Get-Item -LiteralPath $doc100kPath).Length
        Assert-True ($doc100kBytes -ge 98000) "100KB 典型样本字节数应 >= 98KB，实际=$doc100kBytes"

        $session = Start-AppInstance -Context $ctx -Arguments @($doc100kPath) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 500
        $mDoc100k = Get-ProcessTreeMetrics -Process $session.Process

        # 3. 执行阅读 <-> 修改切换与保存回读后采样稳定态内存
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "未能进入修改模式" -Condition {
            $session.MainWindow.Current.Name -match '修改模式'
        }
        $editor = Find-Control -Root $session.MainWindow -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT|RichEdit" -MinHeight 80 -TimeoutMs 5000
        $editHwnd = [IntPtr]$editor.Current.NativeWindowHandle
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $editHwnd -Ctrl -Vk 0x23 -Extended
        Start-Sleep -Milliseconds 100
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $editor -Text "`n<!-- T4_06_MEM_STABLE -->" -PreserveUndo
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $editHwnd -Ctrl -Vk 0x53
        $null = Wait-Condition -TimeoutMs $CaseEnv.TimeoutMs -ErrorMessage "保存后未切回阅读模式" -Condition {
            $t = $session.MainWindow.Current.Name
            ($t -match '阅读模式') -and ($t -notmatch '\*')
        }
        Start-Sleep -Milliseconds 400
        $mAfterToggle = Get-ProcessTreeMetrics -Process $session.Process

        # 进程隔离与无 WebView2/Node 膨胀核查
        foreach ($m in @($mCold, $mDoc100k, $mAfterToggle)) {
            Assert-Equal -Actual $m.ProcessCount -Expected 1 -Message "应用必须保持纯原生单进程 (ProcessCount=1)"
            foreach ($cn in $m.ChildNames) {
                Assert-True ($cn -notmatch 'msedgewebview2|node') "严禁派生高内存 WebView2/Node 子进程: $cn"
            }
            Assert-True ($m.HandleCount -gt 0) "HandleCount 应有效采集 (>0)"
        }

        # 严格区分真实被测程序与 -SelfTest WinForms CLR 替身的内存阈值
        if (-not $CaseEnv.IsSelfTest) {
            Assert-True ($mCold.WorkingSetMB -le 50) "真实 exe 冷启动 WorkingSet 必须 <= 50 MB，实际=$($mCold.WorkingSetMB) MB"
            Assert-True ($mDoc100k.WorkingSetMB -le 60) "真实 exe 打开 100KB 文档 WorkingSet 必须 <= 60 MB，实际=$($mDoc100k.WorkingSetMB) MB"
            Assert-True ($mAfterToggle.WorkingSetMB -le 65) "真实 exe 切换保存回读后 WorkingSet 应 <= 65 MB，实际=$($mAfterToggle.WorkingSetMB) MB"
        } else {
            Assert-True ($mCold.WorkingSetMB -gt 0 -and $mCold.WorkingSetMB -lt 120) "SelfTest 冷启动 WorkingSet 应正常采集且 < 120 MB，实际=$($mCold.WorkingSetMB) MB"
            Assert-True ($mDoc100k.WorkingSetMB -gt 0 -and $mDoc100k.WorkingSetMB -lt 120) "SelfTest 100KB 文档 WorkingSet 应 < 120 MB，实际=$($mDoc100k.WorkingSetMB) MB"
            Assert-True ($mAfterToggle.WorkingSetMB -gt 0 -and $mAfterToggle.WorkingSetMB -lt 120) "SelfTest 切换回读后 WorkingSet 应 < 120 MB，实际=$($mAfterToggle.WorkingSetMB) MB"
        }

        return @{
            Metrics = @{
                WorkingSetMB            = $mDoc100k.WorkingSetMB
                PrivateMemoryMB         = $mDoc100k.PrivateMemoryMB
                HandleCount             = $mDoc100k.HandleCount
                ProcessCount            = $mDoc100k.ProcessCount
                ColdWorkingSetMB        = $mCold.WorkingSetMB
                ColdPrivateMemoryMB     = $mCold.PrivateMemoryMB
                AfterToggleWorkingSetMB = $mAfterToggle.WorkingSetMB
                Doc100kBytes            = $doc100kBytes
                ToolbarButtonCount      = $btnCount
                ViewportRatio           = $viewportRatio
            }
        }
    } finally {
        if ($null -ne $session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

return $cases.ToArray()
