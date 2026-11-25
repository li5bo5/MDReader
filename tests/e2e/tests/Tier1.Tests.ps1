#Requires -Version 5.1
<#
.SYNOPSIS
    MDReader 端到端自动化黑盒测试用例 — Tier 1（核心功能覆盖：60 条，F1–F12 每项 5 条）
.DESCRIPTION
    依据 ORIGINAL_REQUEST.md 与 PROJECT.md 外部契约编写，覆盖 CLI、HKCU 注册表关联、
    文件打开/拖放、GFM 阅读渲染、明暗主题、查找/划词/导航、阅读/修改一键切换、
    RICHEDIT50W 编辑撤销、原子保存自动回读、极简 UI/JSON 配置及 Working Set 内存基线。
#>

Set-StrictMode -Version Latest

# ==============================================================================
# 内部辅助函数（仅供 Tier 1 测试用例复用，严禁产生多余管道输出）
# ==============================================================================

function global:T1_GetPlanContent {
    param([Parameter(Mandatory = $true)][hashtable]$CaseEnv)
    $planPath = Join-Path $CaseEnv.RepoRoot "IMPLEMENTATION_PLAN.md"
    Assert-True (Test-Path -LiteralPath $planPath -PathType Leaf) "缺少实施方案文档: $planPath"
    $meta = Read-FileBytesMeta -Path $planPath
    Assert-True ($meta.Length -gt 0) "IMPLEMENTATION_PLAN.md 不应为空文件"
    return $meta.Text
}

function global:T1_WaitTitleMatch {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Window,
        [Parameter(Mandatory = $true)][string]$Pattern,
        [int]$TimeoutMs = 6000,
        [string]$ErrorMessage = "等待窗口标题变更超时"
    )
    return (Wait-Condition -TimeoutMs $TimeoutMs -ErrorMessage $ErrorMessage -Condition {
        $name = $Window.Current.Name
        if ($name -match $Pattern) { return $name }
        return $null
    })
}

function global:T1_FindModeToggleButton {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Window,
        [int]$TimeoutMs = 5000
    )
    return (Find-Control -Root $Window -AutomationId "IDC_MODE_TOGGLE" -FallbackNames @("编辑", "保存并阅读") -ExcludeTitleBar -TimeoutMs $TimeoutMs)
}

function global:T1_FindCancelButton {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Window,
        [int]$TimeoutMs = 3000
    )
    return (Find-Control -Root $Window -AutomationId "IDC_EDIT_CANCEL" -FallbackNames @("取消", "返回") -ExcludeTitleBar -TimeoutMs $TimeoutMs)
}

function global:T1_FindOpenButton {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Window,
        [int]$TimeoutMs = 5000
    )
    return (Find-Control -Root $Window -AutomationId "IDC_FILE_OPEN" -FallbackNames @("打开") -ExcludeTitleBar -TimeoutMs $TimeoutMs)
}

function global:T1_FindEditorControl {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Window,
        [int]$TimeoutMs = 4000
    )
    return (Find-Control -Root $Window -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT50W|RichEdit20W|RichEdit" -MinHeight 80 -ExcludeTitleBar -TimeoutMs $TimeoutMs)
}

function global:T1_SetOpenFileDialogPathAndAccept {
    param(
        [Parameter(Mandatory = $true)][System.Windows.Automation.AutomationElement]$Dialog,
        [Parameter(Mandatory = $true)][string]$FilePath
    )
    $dlgHwnd = [IntPtr]$Dialog.Current.NativeWindowHandle
    $edit = Find-Control -Root $Dialog -AutomationId "1148" -ClassRegex "Edit" -TimeoutMs 3000
    if ($null -eq $edit) {
        $edit = Find-Control -Root $Dialog -ClassRegex "^Edit$" -TimeoutMs 2500
    }
    Assert-True ($null -ne $edit) "系统文件对话框内未找到文件名输入框"

    $setDone = $false
    $vp = $null
    try {
        if ($edit.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$vp)) {
            if (-not $vp.Current.IsReadOnly) {
                $vp.SetValue($FilePath)
                $setDone = $true
            }
        }
    } catch {}
    if (-not $setDone) {
        Set-EditorText -MainHwnd $dlgHwnd -EditorElement $edit -Text $FilePath -ReplaceAll -PreserveUndo:$false
    }
    Start-Sleep -Milliseconds 120
    Invoke-ModalBucket -Modal $Dialog -Bucket Ok
}

function global:T1_NormalizeEol {
    param([AllowEmptyString()][string]$Text)
    if ($null -eq $Text) { return "" }
    return (($Text -replace "`r`n", "`n") -replace "`r", "`n")
}

function global:Get-SampleTempShot {
    param([string]$Prefix = "shot")
    return (Join-Path $env:TEMP ("mdreader_shot_" + $Prefix + "_" + [Guid]::NewGuid().ToString("N").Substring(0, 8) + ".png"))
}

function global:T1_AssertClientViewportClean {
    param(
        [Parameter(Mandatory = $true)][IntPtr]$Hwnd,
        [Parameter(Mandatory = $true)][string]$WorkDir
    )
    $png = Join-Path $WorkDir ("render_" + [Guid]::NewGuid().ToString("N").Substring(0, 6) + ".png")
    $null = Capture-WindowPng -Hwnd $Hwnd -OutPath $png
    $bmp = [System.Drawing.Bitmap]::FromFile($png)
    try {
        Assert-True ($bmp.Width -gt 200 -and $bmp.Height -gt 150) "截图尺寸异常: $($bmp.Width)x$($bmp.Height)"
        # 采样客户区正文区域像素（跳过顶部约 42px 工具栏，覆盖首行标题至正文底端）
        $x0 = [int]($bmp.Width * 0.12); $x1 = [int]($bmp.Width * 0.88)
        $y0 = [int]([math]::Max(46, $bmp.Height * 0.10)); $y1 = [int]($bmp.Height * 0.88)
        $minLum = 255.0
        $maxLum = 0.0
        for ($y = $y0; $y -lt $y1; $y += 3) {
            for ($x = $x0; $x -lt $x1; $x += 3) {
                $c = $bmp.GetPixel($x, $y)
                $lum = 0.2126 * $c.R + 0.7152 * $c.G + 0.0722 * $c.B
                if ($lum -lt $minLum) { $minLum = $lum }
                if ($lum -gt $maxLum) { $maxLum = $lum }
            }
        }
        Assert-True (($maxLum - $minLum) -gt 15) "阅读画布呈纯单色死屏 (minLum=$minLum, maxLum=$maxLum)"
    } finally {
        $bmp.Dispose()
    }
}

$cases = New-Object 'System.Collections.Generic.List[object]'

# ==============================================================================
# F1: 实施方案文档输出 (T1-F1-01 .. T1-F1-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T1-F1-01" -Tier 1 -Feature "F1" -Group "G1" -Title "核查实施方案文档存在且 UTF-8 非空" -Run {
    param($CaseEnv)
    $text = T1_GetPlanContent -CaseEnv $CaseEnv
    Assert-Match -Text $text -Pattern "MDReader|Markdown" -Message "方案文档缺少项目主题描述"
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F1-02" -Tier 1 -Feature "F1" -Group "G1" -Title "方案包含内存>UI美化>数据安全优先级与选型分析" -Run {
    param($CaseEnv)
    $text = T1_GetPlanContent -CaseEnv $CaseEnv
    Assert-Match -Text $text -Pattern "内存\s*>\s*(?:\d+\.\s*)?UI\s*美化\s*>\s*(?:\d+\.\s*)?数据安全" -Message "缺少核心优先级原则声明"
    Assert-Match -Text $text -Pattern "Direct2D|DirectWrite|WebView2|Electron" -Message "缺少技术栈对比论证"
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F1-03" -Tier 1 -Feature "F1" -Group "G1" -Title "方案包含对标参考产品借鉴与取舍矩阵" -Run {
    param($CaseEnv)
    $text = T1_GetPlanContent -CaseEnv $CaseEnv
    Assert-Match -Text $text -Pattern "mdfiler\.com" -Message "缺少 mdfiler.com 阅读体验参考"
    Assert-Match -Text $text -Pattern "Shazao-markdown" -Message "缺少 Shazao-markdown 参考取舍"
    Assert-Match -Text $text -Pattern "smartpdf|SumatraPDF" -Message "缺少 smartpdf 极简设置参考"
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F1-04" -Tier 1 -Feature "F1" -Group "G1" -Title "方案包含架构分层、模式流转及 HKCU .md 关联设计" -Run {
    param($CaseEnv)
    $text = T1_GetPlanContent -CaseEnv $CaseEnv
    Assert-Match -Text $text -Pattern "Reading|阅读模式" -Message "缺少阅读模式流转设计"
    Assert-Match -Text $text -Pattern "Editing|修改模式" -Message "缺少修改模式流转设计"
    Assert-Match -Text $text -Pattern "MDReader\.Document" -Message "缺少 HKCU ProgID 关联设计"
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F1-05" -Tier 1 -Feature "F1" -Group "G1" -Title "方案包含单一本地 JSON 配置设计及验证计划" -Run {
    param($CaseEnv)
    $text = T1_GetPlanContent -CaseEnv $CaseEnv
    Assert-Match -Text $text -Pattern "config\.json" -Message "缺少 config.json 配置设计"
    Assert-Match -Text $text -Pattern "WorkingSet|Working Set" -Message "缺少 Working Set 内存验证计划"
}))

# ==============================================================================
# F2: Windows .md 文件关联注册与卸载 (T1-F2-01 .. T1-F2-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T1-F2-01" -Tier 1 -Feature "F2" -Group "G2" -Title "--register 写入 HKCU MDReader.Document 与 Capabilities" -Run {
    param($CaseEnv)
    $snap = Enter-HkcuRegistrySandbox
    $ctx = New-E2ECaseContext -CaseId "T1-F2-01" -ExePath $CaseEnv.ExePath -CopyExeToSandbox -RunId $CaseEnv.RunId
    try {
        $res = Invoke-AppCli -ExePath $ctx.ExePath -Arguments @("--register") -TimeoutMs $CaseEnv.TimeoutMs
        Assert-Equal -Actual $res.ExitCode -Expected 0 -Message "--register 应返回退出码 0 (StdErr=$($res.StdErr))"
        Assert-True (-not $res.HadUnexpectedModal) "--register 不应弹出模态窗口"

        Assert-True (Test-Path -LiteralPath "Registry::HKCU\Software\Classes\MDReader.Document") "缺少 ProgID 主键"
        Assert-True (Test-Path -LiteralPath "Registry::HKCU\Software\Classes\MDReader.Document\shell\open\command") "缺少 open\command 键"
        Assert-True (Test-Path -LiteralPath "Registry::HKCU\Software\MDReader\Capabilities\FileAssociations") "缺少 Capabilities\FileAssociations 键"
        $regApps = Get-ItemProperty -LiteralPath "Registry::HKCU\Software\RegisteredApplications" -ErrorAction SilentlyContinue
        Assert-True ($null -ne $regApps -and ($regApps.PSObject.Properties.Name -contains "MDReader")) "RegisteredApplications 缺少 MDReader 项"
    } finally {
        Restore-HkcuRegistrySandbox -Snapshot $snap
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F2-02" -Tier 1 -Feature "F2" -Group "G2" -Title "--register 写入 .md 与 .markdown OpenWithProgids" -Run {
    param($CaseEnv)
    $snap = Enter-HkcuRegistrySandbox
    $ctx = New-E2ECaseContext -CaseId "T1-F2-02" -ExePath $CaseEnv.ExePath -CopyExeToSandbox -RunId $CaseEnv.RunId
    try {
        $res = Invoke-AppCli -ExePath $ctx.ExePath -Arguments @("--register") -TimeoutMs $CaseEnv.TimeoutMs
        Assert-Equal -Actual $res.ExitCode -Expected 0 -Message "--register 失败"

        foreach ($ext in @(".md", ".markdown")) {
            $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\Classes\$ext\OpenWithProgids", $false)
            try {
                Assert-True ($null -ne $k) "缺少 $ext\OpenWithProgids 键"
                Assert-True ($k.GetValueNames() -contains "MDReader.Document") "$ext\OpenWithProgids 未包含 MDReader.Document"
            } finally {
                if ($k) { $k.Dispose() }
            }
        }
    } finally {
        Restore-HkcuRegistrySandbox -Snapshot $snap
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F2-03" -Tier 1 -Feature "F2" -Group "G2" -Title "open command 模板格式严格匹配双引号路径与 %1" -Run {
    param($CaseEnv)
    $snap = Enter-HkcuRegistrySandbox
    $ctx = New-E2ECaseContext -CaseId "T1-F2-03" -ExePath $CaseEnv.ExePath -CopyExeToSandbox -RunId $CaseEnv.RunId
    try {
        $res = Invoke-AppCli -ExePath $ctx.ExePath -Arguments @("--register") -TimeoutMs $CaseEnv.TimeoutMs
        Assert-Equal -Actual $res.ExitCode -Expected 0 -Message "--register 失败"

        $cmdVal = (Get-ItemProperty -LiteralPath "Registry::HKCU\Software\Classes\MDReader.Document\shell\open\command").'(default)'
        $expectedCmd = '"' + $ctx.ExePath + '" "%1"'
        Assert-Equal -Actual $cmdVal -Expected $expectedCmd -Message "shell\open\command 命令行模板不符合契约"
    } finally {
        Restore-HkcuRegistrySandbox -Snapshot $snap
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F2-04" -Tier 1 -Feature "F2" -Group "G2" -Title "--unregister 精准清理自建键值且不误伤第三方值" -Run {
    param($CaseEnv)
    $snap = Enter-HkcuRegistrySandbox
    $ctx = New-E2ECaseContext -CaseId "T1-F2-04" -ExePath $CaseEnv.ExePath -CopyExeToSandbox -RunId $CaseEnv.RunId
    try {
        $owp = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Software\Classes\.md\OpenWithProgids")
        try { $owp.SetValue("ThirdPartyMd.Sentinel", "", [Microsoft.Win32.RegistryValueKind]::String) } finally { $owp.Dispose() }

        $r1 = Invoke-AppCli -ExePath $ctx.ExePath -Arguments @("--register") -TimeoutMs $CaseEnv.TimeoutMs
        Assert-Equal -Actual $r1.ExitCode -Expected 0 -Message "--register 失败"

        $r2 = Invoke-AppCli -ExePath $ctx.ExePath -Arguments @("--unregister") -TimeoutMs $CaseEnv.TimeoutMs
        Assert-Equal -Actual $r2.ExitCode -Expected 0 -Message "--unregister 失败 (StdErr=$($r2.StdErr))"

        Assert-True (-not (Test-Path -LiteralPath "Registry::HKCU\Software\Classes\MDReader.Document")) "MDReader.Document 应已清理"
        Assert-True (-not (Test-Path -LiteralPath "Registry::HKCU\Software\MDReader")) "HKCU\Software\MDReader 应已清理"

        $owpCheck = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\Classes\.md\OpenWithProgids", $false)
        try {
            Assert-True ($null -ne $owpCheck) ".md\OpenWithProgids 不应被整键误删"
            $names = $owpCheck.GetValueNames()
            Assert-True (-not ($names -contains "MDReader.Document")) "OpenWithProgids 中的 MDReader.Document 应已移除"
            Assert-True ($names -contains "ThirdPartyMd.Sentinel") "第三方关联值不应被误删"
        } finally {
            if ($owpCheck) { $owpCheck.Dispose() }
        }
    } finally {
        Restore-HkcuRegistrySandbox -Snapshot $snap
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F2-05" -Tier 1 -Feature "F2" -Group "G2" -Title "scripts/register 与 unregister PS1 脚本 AST 与沙盒执行" -Run {
    param($CaseEnv)
    $snap = Enter-HkcuRegistrySandbox
    $ctx = New-E2ECaseContext -CaseId "T1-F2-05" -ExePath $CaseEnv.ExePath -CopyExeToSandbox -RunId $CaseEnv.RunId
    try {
        $regScript = Join-Path $CaseEnv.RepoRoot "scripts\register-md-assoc.ps1"
        $unregScript = Join-Path $CaseEnv.RepoRoot "scripts\unregister-md-assoc.ps1"
        $hasRealScripts = (Test-Path -LiteralPath $regScript -PathType Leaf) -and (Test-Path -LiteralPath $unregScript -PathType Leaf)

        if (-not $hasRealScripts) {
            Assert-True $CaseEnv.IsSelfTest "真实被测模式下缺少 $regScript 或 $unregScript"
            $regScript = Join-Path $ctx.SandboxDir "register-md-assoc.ps1"
            $unregScript = Join-Path $ctx.SandboxDir "unregister-md-assoc.ps1"
            $regBody = @"
param([string]`$ExePath = '$($ctx.ExePath)')
& `$ExePath --register
exit `$LASTEXITCODE
"@
            $unregBody = @"
param([string]`$ExePath = '$($ctx.ExePath)')
& `$ExePath --unregister
exit `$LASTEXITCODE
"@
            Write-BytesUtf8 -Path $regScript -Text $regBody -Bom $false -Eol 'CRLF'
            Write-BytesUtf8 -Path $unregScript -Text $unregBody -Bom $false -Eol 'CRLF'
            $CaseEnv.Metrics.Note = "M3 scripts pending"
        }

        foreach ($sp in @($regScript, $unregScript)) {
            $tokens = $null; $errs = $null
            $null = [System.Management.Automation.Language.Parser]::ParseFile($sp, [ref]$tokens, [ref]$errs)
            Assert-True ($null -eq $errs -or $errs.Count -eq 0) "脚本 $sp 存在语法错误"
        }

        $regCmd = Get-Command $regScript
        if ($regCmd.Parameters.ContainsKey("ExePath")) {
            $null = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $regScript -ExePath $ctx.ExePath
        } else {
            $null = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $regScript
        }
        Assert-Equal -Actual $LASTEXITCODE -Expected 0 -Message "执行 $regScript 失败 (Exit=$LASTEXITCODE)"
        Assert-True (Test-Path -LiteralPath "Registry::HKCU\Software\Classes\MDReader.Document\shell\open\command") "注册脚本未写入 HKCU ProgID"

        $unregCmd = Get-Command $unregScript
        if ($unregCmd.Parameters.ContainsKey("ExePath")) {
            $null = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $unregScript -ExePath $ctx.ExePath
        } else {
            $null = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $unregScript
        }
        Assert-Equal -Actual $LASTEXITCODE -Expected 0 -Message "执行 $unregScript 失败 (Exit=$LASTEXITCODE)"
        Assert-True (-not (Test-Path -LiteralPath "Registry::HKCU\Software\Classes\MDReader.Document")) "卸载脚本未清理 HKCU ProgID"
    } finally {
        Restore-HkcuRegistrySandbox -Snapshot $snap
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# F3: 命令行路径与多参数启动 (T1-F3-01 .. T1-F3-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T1-F3-01" -Tier 1 -Feature "F3" -Group "G4" -Title "命令行传入 .md 路径直接启动进入阅读模式" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F3-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "hello.md"
        Write-BytesUtf8 -Path $doc -Text "# Hello MDReader`n`nCLI direct open test."
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        $btn = T1_FindModeToggleButton -Window $session.MainWindow
        Assert-True ($null -ne $btn) "缺少模式切换按钮 IDC_MODE_TOGGLE"
        Assert-Equal -Actual $btn.Current.Name -Expected "编辑" -Message "直接启动应默认处于阅读模式"

        $ed = T1_FindEditorControl -Window $session.MainWindow -TimeoutMs 400
        Assert-True ($null -eq $ed) "阅读模式下编辑控件不应可见"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F3-02" -Tier 1 -Feature "F3" -Group "G4" -Title "窗口标题严格符合 <filename> - 阅读模式 - MDReader" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F3-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "alpha_doc.md"
        Write-BytesUtf8 -Path $doc -Text "# Title Check"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        $title = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "alpha_doc\.md\s*-\s*阅读模式\s*-\s*MDReader"
        Assert-Match -Text $title -Pattern "^alpha_doc\.md\s*-\s*阅读模式\s*-\s*MDReader$"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F3-03" -Tier 1 -Feature "F3" -Group "G4" -Title "--config <path> 指定外部配置文件并加载生效" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F3-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "cfg_test.md"
        $cfg = Join-Path $ctx.SandboxDir "custom_dark.json"
        Write-BytesUtf8 -Path $doc -Text "# Config Override`n`nDark theme check."
        Write-BytesUtf8 -Path $cfg -Text '{"theme": "dark", "fontSize": 18}'

        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs
        $lum = Wait-Condition -TimeoutMs 5000 -ErrorMessage "等待暗色背景渲染超时" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -ge 0 -and $l -lt 100) { return $l }
            return $null
        }
        Assert-True ($lum -lt 100) "--config 指定 dark 主题应呈现深色亮度 (实际=$lum)"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F3-04" -Tier 1 -Feature "F3" -Group "G4" -Title "无参数启动进入极简空状态欢迎阅读视图" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F3-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $session = Start-AppInstance -Context $ctx -Arguments @() -TimeoutMs $CaseEnv.TimeoutMs
        $title = $session.MainWindow.Current.Name
        Assert-Match -Text $title -Pattern "阅读模式\s*-\s*MDReader" -Message "无参启动应进入阅读模式"
        $modal = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 400
        Assert-True ($null -eq $modal) "无参启动不应弹出阻塞模态框"
        T1_AssertClientViewportClean -Hwnd $session.Hwnd -WorkDir $ctx.SandboxDir
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F3-05" -Tier 1 -Feature "F3" -Group "G2" -Title "--help 输出命令行帮助并正常返回退出码 0" -Run {
    param($CaseEnv)
    $res = Invoke-AppCli -ExePath $CaseEnv.ExePath -Arguments @("--help") -TimeoutMs $CaseEnv.TimeoutMs
    Assert-Equal -Actual $res.ExitCode -Expected 0 -Message "--help 应返回退出码 0"
    Assert-True (-not $res.TimedOut) "--help 不应超时"
    Assert-True (-not $res.HadUnexpectedModal) "--help 不应弹出窗口"
    Assert-Match -Text ($res.StdOut + "`n" + $res.StdErr) -Pattern "--config|--register|MDReader" -Message "--help 输出缺少关键开关说明"
}))

# ==============================================================================
# F4: 拖放与文件对话框打开 (T1-F4-01 .. T1-F4-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T1-F4-01" -Tier 1 -Feature "F4" -Group "G4" -Title "阅读态点击极简打开按钮 IDC_FILE_OPEN 唤起文件对话框" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F4-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "open_btn.md"
        Write-BytesUtf8 -Path $doc -Text "# Open Button Test"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        $openBtn = T1_FindOpenButton -Window $session.MainWindow
        Assert-True ($null -ne $openBtn) "未找到极简打开按钮 IDC_FILE_OPEN"
        Invoke-Button -Element $openBtn -MainHwnd $session.Hwnd

        $dlg = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 5000
        Assert-True ($null -ne $dlg) "点击打开按钮后未弹出系统文件选择对话框"
        Invoke-ModalBucket -Modal $dlg -Bucket Cancel
        $null = Wait-Condition -TimeoutMs 4000 -ErrorMessage "关闭文件对话框超时" -Condition {
            $null -eq (Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 0)
        }
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F4-02" -Tier 1 -Feature "F4" -Group "G4" -Title "Ctrl+O 唤起文件对话框后取消保持原文档不变" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F4-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "keep_doc.md"
        Write-BytesUtf8 -Path $doc -Text "# Keep Content Line"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x4F # Ctrl+O
        $dlg = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 5000
        Assert-True ($null -ne $dlg) "Ctrl+O 未弹出文件对话框"
        Invoke-ModalBucket -Modal $dlg -Bucket Cancel
        $null = Wait-Condition -TimeoutMs 4000 -ErrorMessage "对话框未关闭" -Condition {
            $null -eq (Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 0)
        }
        Assert-Match -Text $session.MainWindow.Current.Name -Pattern "keep_doc\.md" -Message "取消打开后原文档标题不应变化"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F4-03" -Tier 1 -Feature "F4" -Group "G4" -Title "Ctrl+O 通过系统文件对话框选择并切换打开新 .md 文件" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F4-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $docA = Join-Path $ctx.SandboxDir "docA.md"
        $docB = Join-Path $ctx.SandboxDir "docB.md"
        Write-BytesUtf8 -Path $docA -Text "# Document A"
        Write-BytesUtf8 -Path $docB -Text "# Document B Switched"

        $session = Start-AppInstance -Context $ctx -Arguments @($docA) -TimeoutMs $CaseEnv.TimeoutMs
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x4F # Ctrl+O
        $dlg = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 5000
        Assert-True ($null -ne $dlg) "Ctrl+O 未弹出文件对话框"

        T1_SetOpenFileDialogPathAndAccept -Dialog $dlg -FilePath $docB
        $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "docB\.md\s*-\s*阅读模式" -TimeoutMs 6000 -ErrorMessage "通过文件对话框打开 docB.md 后标题未更新"

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45 # Ctrl+E 核验内容
        $ed = T1_FindEditorControl -Window $session.MainWindow -TimeoutMs 4000
        Assert-True ($null -ne $ed) "切入编辑态失败"
        Assert-Match -Text (Get-EditorText -EditorElement $ed) -Pattern "Document B Switched"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F4-04" -Tier 1 -Feature "F4" -Group "G4" -Title "主窗口具备拖放接收能力（WS_EX_ACCEPTFILES / WM_DROPFILES）" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F4-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "accept_drop.md"
        Write-BytesUtf8 -Path $doc -Text "# Accept Drop Verified"
        $session = Start-AppInstance -Context $ctx -Arguments @() -TimeoutMs $CaseEnv.TimeoutMs
        $exStyle = [E2ENative]::GetWindowExStyle($session.Hwnd)
        $hasWsEx = (($exStyle -band [E2ENative]::WS_EX_ACCEPTFILES) -ne 0)
        $CaseEnv.Metrics.HasWsExAcceptFiles = $hasWsEx
        if (-not $CaseEnv.IsSelfTest) {
            Assert-True $hasWsEx "真实主窗口缺少 WS_EX_ACCEPTFILES 扩展样式 (ExStyle=$exStyle)"
        } else {
            # WinForms AllowDrop=true 会接管 OLE 拖放从而清除 WS_EX_ACCEPTFILES，直接验证 WM_DROPFILES 接收有效
            Send-DropFiles -Hwnd $session.Hwnd -Paths @($doc)
            $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "accept_drop\.md"
        }
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F4-05" -Tier 1 -Feature "F4" -Group "G4" -Title "WM_DROPFILES 拖入新 .md 文件后切入目标文档阅读态" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F4-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $docA = Join-Path $ctx.SandboxDir "first.md"
        $docB = Join-Path $ctx.SandboxDir "dropped_target.md"
        Write-BytesUtf8 -Path $docA -Text "# First"
        Write-BytesUtf8 -Path $docB -Text "# Dropped Success Content"

        $session = Start-AppInstance -Context $ctx -Arguments @($docA) -TimeoutMs $CaseEnv.TimeoutMs
        Send-DropFiles -Hwnd $session.Hwnd -Paths @($docB)

        $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "dropped_target\.md\s*-\s*阅读模式" -TimeoutMs 6000 -ErrorMessage "拖放文件后窗口标题未切换到新文档"
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T1_FindEditorControl -Window $session.MainWindow
        Assert-True ($null -ne $ed) "未找到编辑器"
        Assert-Match -Text (Get-EditorText -EditorElement $ed) -Pattern "Dropped Success Content"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# F5: 纯粹阅读模式 GFM 渲染 (T1-F5-01 .. T1-F5-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T1-F5-01" -Tier 1 -Feature "F5" -Group "G4" -Title "渲染 H1-H6 各级标题与段落文本" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F5-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "headings.md"
        Write-BytesUtf8 -Path $doc -Text "# H1 Title`n## H2 Title`n### H3 Title`n#### H4 Title`n##### H5 Title`n###### H6 Title`n`nNormal paragraph with 中文混排 text."
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 250
        Assert-True (-not $session.Process.HasExited) "进程意外退出"
        T1_AssertClientViewportClean -Hwnd $session.Hwnd -WorkDir $ctx.SandboxDir
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F5-02" -Tier 1 -Feature "F5" -Group "G4" -Title "渲染粗体/斜体/删除线/分割线与有序无序列表" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F5-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $src = Join-Path $CaseEnv.FixturesDir "sample-basic.md"
        $doc = Join-Path $ctx.SandboxDir "sample-basic.md"
        Copy-Item -LiteralPath $src -Destination $doc
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 250
        Assert-True (-not $session.Process.HasExited) "渲染 sample-basic.md 异常退出"
        T1_AssertClientViewportClean -Hwnd $session.Hwnd -WorkDir $ctx.SandboxDir
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F5-03" -Tier 1 -Feature "F5" -Group "G4" -Title "渲染 GFM 任务复选框与多列对齐表格" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F5-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "tasks_table.md"
        $md = @"
# 任务与表格

- [x] 已完成任务
- [ ] 待办任务

| 列左 | 列中 | 列右 |
|:---|:---:|---:|
| A1 | B1 | 100 |
| A2 | B2 | 200 |
"@
        Write-BytesUtf8 -Path $doc -Text $md
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 250
        Assert-True (-not $session.Process.HasExited) "渲染任务清单与表格失败"
        T1_AssertClientViewportClean -Hwnd $session.Hwnd -WorkDir $ctx.SandboxDir
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F5-04" -Tier 1 -Feature "F5" -Group "G4" -Title "渲染多语言围栏代码块与多级引用块" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F5-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "code_quote.md"
        $md = @"
> 一级引用文本
> > 二级引用文本

``````rust
fn greet(name: &str) -> String {
    format!("Hello, {}", name)
}
``````
"@
        Write-BytesUtf8 -Path $doc -Text $md
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 250
        Assert-True (-not $session.Process.HasExited) "渲染代码块与引用块失败"
        T1_AssertClientViewportClean -Hwnd $session.Hwnd -WorkDir $ctx.SandboxDir
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F5-05" -Tier 1 -Feature "F5" -Group "G4" -Title "渲染含本地相对路径合法 PNG 图片与外链全量 GFM 文档" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F5-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "sample-gfm-full.md"
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "sample-gfm-full.md") -Destination $doc
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "test-image.png") -Destination (Join-Path $ctx.SandboxDir "test-image.png")

        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 350
        Assert-True (-not $session.Process.HasExited) "渲染含本地图片全量 GFM 失败"
        $modal = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 300
        Assert-True ($null -eq $modal) "不应弹出异常弹窗"
        T1_AssertClientViewportClean -Hwnd $session.Hwnd -WorkDir $ctx.SandboxDir
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# F6: 亮 / 暗 / 跟随系统主题渲染 (T1-F6-01 .. T1-F6-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T1-F6-01" -Tier 1 -Feature "F6" -Group "G4" -Title "theme=light 呈现高亮度浅色阅读背景" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F6-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "light.md"
        $cfg = Join-Path $ctx.SandboxDir "light.json"
        Write-BytesUtf8 -Path $doc -Text "# Light Theme"
        Write-BytesUtf8 -Path $cfg -Text '{"theme": "light"}'
        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs
        $lum = Wait-Condition -TimeoutMs 5000 -ErrorMessage "亮色主题亮度采样未达标" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -ge 170) { return $l }
            return $null
        }
        $CaseEnv.Metrics.LuminanceLight = [math]::Round($lum, 1)
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F6-02" -Tier 1 -Feature "F6" -Group "G4" -Title "theme=dark 呈现低眩光深色阅读背景" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F6-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "dark.md"
        $cfg = Join-Path $ctx.SandboxDir "dark.json"
        Write-BytesUtf8 -Path $doc -Text "# Dark Theme"
        Write-BytesUtf8 -Path $cfg -Text '{"theme": "dark"}'
        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs
        $lum = Wait-Condition -TimeoutMs 5000 -ErrorMessage "暗色主题亮度采样未达标" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -ge 0 -and $l -le 95) { return $l }
            return $null
        }
        $CaseEnv.Metrics.LuminanceDark = [math]::Round($lum, 1)
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F6-03" -Tier 1 -Feature "F6" -Group "G4" -Title "theme=system 跟随系统主题正常启动渲染" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F6-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "sys.md"
        $cfg = Join-Path $ctx.SandboxDir "sys.json"
        Write-BytesUtf8 -Path $doc -Text "# System Theme`n`nFollowing system light/dark preference with clean Markdown body content."
        Write-BytesUtf8 -Path $cfg -Text '{"theme": "system"}'
        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 250
        $lum = Get-WindowLuminance -Hwnd $session.Hwnd
        Assert-True ($lum -ge 0) "theme=system 窗口采样失败"
        T1_AssertClientViewportClean -Hwnd $session.Hwnd -WorkDir $ctx.SandboxDir
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F6-04" -Tier 1 -Feature "F6" -Group "G4" -Title "暗色主题下阅读区与修改区底色一致性检查" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F6-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "theme_sync.md"
        $cfg = Join-Path $ctx.SandboxDir "theme_sync.json"
        Write-BytesUtf8 -Path $doc -Text "# Sync Theme`n`nLine 1`nLine 2"
        Write-BytesUtf8 -Path $cfg -Text '{"theme": "dark"}'
        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs

        $lumRead = Wait-Condition -TimeoutMs 5000 -ErrorMessage "阅读区暗色未生效" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -ge 0 -and $l -le 95) { return $l }
            return $null
        }
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T1_FindEditorControl -Window $session.MainWindow -TimeoutMs 4000
        Assert-True ($null -ne $ed) "未进入修改模式"
        $lumEdit = Wait-Condition -TimeoutMs 4000 -ErrorMessage "编辑区底色非暗色 (应与暗色主题一致)" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -ge 0 -and $l -le 110) { return $l }
            return $null
        }
        $CaseEnv.Metrics.LumRead = [math]::Round($lumRead, 1)
        $CaseEnv.Metrics.LumEdit = [math]::Round($lumEdit, 1)
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F6-05" -Tier 1 -Feature "F6" -Group "G4" -Title "显式 colors 自定义配色覆盖生效" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F6-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "custom_colors.md"
        $cfg = Join-Path $ctx.SandboxDir "custom_colors.json"
        Write-BytesUtf8 -Path $doc -Text "# Custom Colors"
        $cfgJson = @'
{
  "theme": "light",
  "colors": {
    "background": "#162030",
    "text": "#E6EDF3",
    "codeBackground": "#1F2D42",
    "quoteBackground": "#1B2638",
    "link": "#58A6FF",
    "border": "#304562"
  }
}
'@
        Write-BytesUtf8 -Path $cfg -Text $cfgJson
        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs
        $lum = Wait-Condition -TimeoutMs 5000 -ErrorMessage "自定义 colors.background 未生效" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -ge 0 -and $l -lt 80) { return $l }
            return $null
        }
        Assert-True ($lum -lt 80) "自定义深蓝背景应覆盖 light 主题默认亮背景 (lum=$lum)"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# F7: 阅读区导航、划词复制与页内查找 (T1-F7-01 .. T1-F7-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T1-F7-01" -Tier 1 -Feature "F7" -Group "G4" -Title "长文档支持 PageDown/End/Home 键盘翻页滚动" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F7-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "sample-long.md"
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "sample-long.md") -Destination $doc
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 300

        $shotTop = Join-Path $ctx.SandboxDir "top.png"
        $shotDown = Join-Path $ctx.SandboxDir "down.png"
        $shotHome = Join-Path $ctx.SandboxDir "home.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotTop

        Send-Hotkey -MainHwnd $session.Hwnd -Vk 0x22 -Extended # PageDown
        Start-Sleep -Milliseconds 200
        Send-Hotkey -MainHwnd $session.Hwnd -Vk 0x23 -Extended # End
        Start-Sleep -Milliseconds 250
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotDown

        $diffScroll = Compare-WindowCaptureDiff -PngPathA $shotTop -PngPathB $shotDown
        Assert-True ($diffScroll -gt 0.005) "PageDown/End 后画面未发生滚动变化 (diff=$diffScroll)"

        Send-Hotkey -MainHwnd $session.Hwnd -Vk 0x24 -Extended # Home
        Start-Sleep -Milliseconds 250
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotHome
        Assert-True (-not $session.Process.HasExited) "Home 滚回顶部后进程异常退出"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F7-02" -Tier 1 -Feature "F7" -Group "G4" -Title "长文档支持鼠标滚轮向下/向上滚动" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F7-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "sample-long.md"
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "sample-long.md") -Destination $doc
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 250

        $s1 = Join-Path $ctx.SandboxDir "w1.png"
        $s2 = Join-Path $ctx.SandboxDir "w2.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $s1

        $readerEl = Find-Control -Root $session.MainWindow -AutomationId "IDC_READER" -MinHeight 100 -ExcludeTitleBar -TimeoutMs 1000
        $wheelHwnd = if ($readerEl -and ([IntPtr]$readerEl.Current.NativeWindowHandle -ne [IntPtr]::Zero)) { [IntPtr]$readerEl.Current.NativeWindowHandle } else { $session.Hwnd }
        Send-MouseWheelScroll -Hwnd $wheelHwnd -Delta -480
        Start-Sleep -Milliseconds 220
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $s2
        $diff = Compare-WindowCaptureDiff -PngPathA $s1 -PngPathB $s2
        Assert-True ($diff -gt 0.002) "鼠标滚轮滚动后画面未变化 (diff=$diff)"
        Assert-True (-not $session.Process.HasExited) "滚轮滚动后进程异常退出"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F7-03" -Tier 1 -Feature "F7" -Group "G4" -Title "阅读区支持鼠标划词与 Ctrl+C 复制到剪贴板" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F7-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "select_copy.md"
        Write-BytesUtf8 -Path $doc -Text "# Selectable Heading Text`n`nHere is enough readable content on the top lines for selection test."
        Set-ClipboardTextWin32 -Text "SENTINEL_BEFORE_COPY"

        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 250

        $readerEl = Find-Control -Root $session.MainWindow -AutomationId "IDC_READER" -MinHeight 100 -ExcludeTitleBar -TimeoutMs 1000
        if ($readerEl -and ([IntPtr]$readerEl.Current.NativeWindowHandle -ne [IntPtr]::Zero)) {
            $readerHwnd = [IntPtr]$readerEl.Current.NativeWindowHandle
            Send-MouseDragSelect -Hwnd $readerHwnd -StartX 40 -StartY 25 -EndX 380 -EndY 95
            Start-Sleep -Milliseconds 150
            Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $readerHwnd -Ctrl -Vk 0x43 # Ctrl+C
        } else {
            Send-MouseDragSelect -Hwnd $session.Hwnd -StartX 130 -StartY 75 -EndX 520 -EndY 145
            Start-Sleep -Milliseconds 150
            Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x43 # Ctrl+C
        }

        $copied = Wait-Condition -TimeoutMs 4000 -ErrorMessage "划词后 Ctrl+C 未写入剪贴板" -Condition {
            $cb = Get-ClipboardTextWin32
            if ($cb -and $cb -ne "SENTINEL_BEFORE_COPY") { return $cb }
            return $null
        }
        Assert-True ($copied.Length -ge 3) "复制到的剪贴板内容过短: '$copied'"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F7-04" -Tier 1 -Feature "F7" -Group "G4" -Title "Ctrl+F 唤起页内查找交互并匹配关键词高亮" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F7-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "find_doc.md"
        Write-BytesUtf8 -Path $doc -Text "# Search Target`n`nLine with NeedleKeyword here.`nAnother NeedleKeyword match."
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 200
        $shotBefore = Join-Path $ctx.SandboxDir "find_before.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotBefore

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x46 # Ctrl+F
        Start-Sleep -Milliseconds 180

        $findBox = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_INPUT" -ClassRegex "Edit|TextBox" -MaxHeight 60 -ExcludeTitleBar -TimeoutMs 1500
        if ($null -ne $findBox) {
            Set-EditorText -MainHwnd $session.Hwnd -EditorElement $findBox -Text "NeedleKeyword" -ReplaceAll -PreserveUndo:$false
        } else {
            foreach ($ch in "NeedleKeyword".ToCharArray()) {
                $vk = [int][char]::ToUpperInvariant($ch)
                Send-Hotkey -MainHwnd $session.Hwnd -Vk $vk
            }
        }
        Start-Sleep -Milliseconds 250

        $findStatus = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_STATUS" -TimeoutMs 800
        if ($null -ne $findStatus) {
            Assert-Match -Text $findStatus.Current.Name -Pattern "1/2|2" -Message "查找状态应显示命中结果"
        }
        $shotAfter = Join-Path $ctx.SandboxDir "find_after.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotAfter
        $diff = Compare-WindowCaptureDiff -PngPathA $shotBefore -PngPathB $shotAfter
        Assert-True ($diff -gt 0.001) "Ctrl+F 查找高亮后画面应产生可见变化"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F7-05" -Tier 1 -Feature "F7" -Group "G4" -Title "页内查找支持 Enter 跳转下一处与 Esc 关闭查找栏" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F7-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "find_next.md"
        Write-BytesUtf8 -Path $doc -Text "# Repeat`n`nAlpha one`nAlpha two`nAlpha three"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x46
        $findBox = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_INPUT" -ClassRegex "Edit|TextBox" -MaxHeight 60 -ExcludeTitleBar -TimeoutMs 1500
        if ($null -ne $findBox) {
            Set-EditorText -MainHwnd $session.Hwnd -EditorElement $findBox -Text "Alpha" -ReplaceAll -PreserveUndo:$false
        }
        Start-Sleep -Milliseconds 180
        $focusH = if ($findBox) { [IntPtr]$findBox.Current.NativeWindowHandle } else { [IntPtr]::Zero }
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $focusH -Vk 0x0D # Enter
        Start-Sleep -Milliseconds 150

        $findStatus = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_STATUS" -TimeoutMs 800
        if ($null -ne $findStatus) {
            Assert-Match -Text $findStatus.Current.Name -Pattern "2/3" -Message "按 Enter 后应跳转至第 2/3 个匹配"
        }

        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $focusH -Vk 0x1B # Esc
        Start-Sleep -Milliseconds 180
        $findBoxAfter = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_INPUT" -ClassRegex "Edit|TextBox" -MaxHeight 60 -ExcludeTitleBar -TimeoutMs 400
        Assert-True ($null -eq $findBoxAfter) "按 Esc 后查找输入框应隐藏关闭"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# F8: 阅读 <-> 修改模式一键切换 (T1-F8-01 .. T1-F8-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T1-F8-01" -Tier 1 -Feature "F8" -Group "G4" -Title "阅读态存在 IDC_MODE_TOGGLE('编辑')且隐藏编辑控件与取消按钮" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F8-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "mode_init.md"
        Write-BytesUtf8 -Path $doc -Text "# Initial Reading"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        $toggle = T1_FindModeToggleButton -Window $session.MainWindow
        Assert-True ($null -ne $toggle) "阅读态未找到模式切换按钮"
        Assert-Equal -Actual $toggle.Current.Name -Expected "编辑"
        Assert-True ($null -eq (T1_FindEditorControl -Window $session.MainWindow -TimeoutMs 300)) "阅读态编辑区应隐藏"
        Assert-True ($null -eq (T1_FindCancelButton -Window $session.MainWindow -TimeoutMs 300)) "阅读态取消按钮应隐藏"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F8-02" -Tier 1 -Feature "F8" -Group "G4" -Title "点击'编辑'按钮切入修改模式并显示'保存并阅读'与'取消'" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F8-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "click_toggle.md"
        Write-BytesUtf8 -Path $doc -Text "# Click Toggle"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        $toggle = T1_FindModeToggleButton -Window $session.MainWindow
        Invoke-Button -Element $toggle -MainHwnd $session.Hwnd

        $ed = T1_FindEditorControl -Window $session.MainWindow -TimeoutMs 4000
        Assert-True ($null -ne $ed) "点击编辑后应显示编辑区控件"
        $cancel = T1_FindCancelButton -Window $session.MainWindow -TimeoutMs 3000
        Assert-True ($null -ne $cancel) "修改模式下应显示取消按钮"
        $toggleAfter = Wait-Condition -TimeoutMs 3000 -ErrorMessage "按钮文案未变为保存并阅读" -Condition {
            $b = T1_FindModeToggleButton -Window $session.MainWindow -TimeoutMs 300
            if ($b -and $b.Current.Name -eq "保存并阅读") { return $b }
            return $null
        }
        Assert-Equal -Actual $toggleAfter.Current.Name -Expected "保存并阅读"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F8-03" -Tier 1 -Feature "F8" -Group "G4" -Title "切换修改态/阅读态时窗口标题实时切换模式描述" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F8-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "title_switch.md"
        Write-BytesUtf8 -Path $doc -Text "# Title Switch"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "title_switch\.md\s*-\s*阅读模式\s*-\s*MDReader"

        Invoke-Button -Element (T1_FindModeToggleButton -Window $session.MainWindow) -MainHwnd $session.Hwnd
        $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "title_switch\.md\s*-\s*修改模式\s*-\s*MDReader"

        $cancelBtn = T1_FindCancelButton -Window $session.MainWindow
        Invoke-Button -Element $cancelBtn -MainHwnd $session.Hwnd
        $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "title_switch\.md\s*-\s*阅读模式\s*-\s*MDReader"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F8-04" -Tier 1 -Feature "F8" -Group "G4" -Title "快捷键 Ctrl+E 一键切换阅读与修改模式" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F8-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "ctrl_e.md"
        Write-BytesUtf8 -Path $doc -Text "# Ctrl+E Toggle"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45 # Ctrl+E
        $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "修改模式\s*-\s*MDReader"
        $ed = T1_FindEditorControl -Window $session.MainWindow
        Assert-True ($null -ne $ed) "Ctrl+E 未显示编辑器"

        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$ed.Current.NativeWindowHandle) -Ctrl -Vk 0x45
        $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "阅读模式\s*-\s*MDReader"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F8-05" -Tier 1 -Feature "F8" -Group "G4" -Title "快捷键 F2 一键切换阅读与修改模式" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F8-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "f2_toggle.md"
        Write-BytesUtf8 -Path $doc -Text "# F2 Toggle"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Vk 0x71 # F2
        $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "修改模式\s*-\s*MDReader"
        $ed = T1_FindEditorControl -Window $session.MainWindow
        Assert-True ($null -ne $ed) "F2 未显示编辑器"

        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$ed.Current.NativeWindowHandle) -Vk 0x71
        $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "阅读模式\s*-\s*MDReader"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# F9: 轻量化纯文本修改模式 (T1-F9-01 .. T1-F9-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T1-F9-01" -Tier 1 -Feature "F9" -Group "G4" -Title "修改模式展示原生 RichEdit 控件并完整加载原文源码" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F9-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $rawText = "# 原文标题`n`n- 列表项 A`n- 列表项 B"
        $doc = Join-Path $ctx.SandboxDir "source_load.md"
        Write-BytesUtf8 -Path $doc -Text $rawText
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T1_FindEditorControl -Window $session.MainWindow -TimeoutMs 4000
        Assert-True ($null -ne $ed) "未找到原生 RichEdit 编辑控件"
        $edText = T1_NormalizeEol (Get-EditorText -EditorElement $ed)
        Assert-Equal -Actual $edText.TrimEnd("`n") -Expected $rawText.TrimEnd("`n") -Message "编辑区加载源码与原文件不一致"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F9-02" -Tier 1 -Feature "F9" -Group "G4" -Title "编辑区支持追加与修改中英文混排文本" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F9-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "edit_bilingual.md"
        Write-BytesUtf8 -Path $doc -Text "# 初始"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T1_FindEditorControl -Window $session.MainWindow
        Assert-True ($null -ne $ed) "缺少编辑器"

        $newText = "# 初始`n`n新增中英文 Mixed Content"
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text $newText -ReplaceAll
        $actual = T1_NormalizeEol (Get-EditorText -EditorElement $ed)
        Assert-Match -Text $actual -Pattern "新增中英文 Mixed Content" -Message "编辑区文本未能读回修改内容"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F9-03" -Tier 1 -Feature "F9" -Group "G4" -Title "输入修改后窗口标题出现 * 未保存脏标记" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F9-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "dirty_mark.md"
        Write-BytesUtf8 -Path $doc -Text "# Clean"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "修改模式"
        Assert-True ($session.MainWindow.Current.Name -notmatch '\*') "刚进入修改模式尚未修改时不应有 * 标记"

        $ed = T1_FindEditorControl -Window $session.MainWindow
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text "# Dirty Now" -ReplaceAll
        $dirtyTitle = T1_WaitTitleMatch -Window $session.MainWindow -Pattern '\*' -ErrorMessage "修改文本后窗口标题未出现 * 脏标记"
        Assert-Match -Text $dirtyTitle -Pattern '\*.*修改模式'
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F9-04" -Tier 1 -Feature "F9" -Group "G4" -Title "修改模式支持 Ctrl+Z 撤销与 Ctrl+Y 重做（保留撤销栈）" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F9-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "undo_redo.md"
        Write-BytesUtf8 -Path $doc -Text "BASE_LINE"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T1_FindEditorControl -Window $session.MainWindow
        Assert-True ($null -ne $ed) "缺少编辑器"

        # 保留撤销栈（不传 -PreserveUndo:$false）
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text "UPDATED_LINE" -ReplaceAll
        $afterEdit = (T1_NormalizeEol (Get-EditorText -EditorElement $ed)).Trim()
        Assert-Equal -Actual $afterEdit -Expected "UPDATED_LINE"

        $edHwnd = [IntPtr]$ed.Current.NativeWindowHandle
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $edHwnd -Ctrl -Vk 0x5A # Ctrl+Z
        $afterUndo = Wait-Condition -TimeoutMs 3000 -ErrorMessage "Ctrl+Z 未能撤销回原文" -Condition {
            $t = (T1_NormalizeEol (Get-EditorText -EditorElement $ed)).Trim()
            if ($t -eq "BASE_LINE") { return $t }
            return $null
        }
        Assert-Equal -Actual $afterUndo -Expected "BASE_LINE"

        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $edHwnd -Ctrl -Vk 0x59 # Ctrl+Y
        $afterRedo = Wait-Condition -TimeoutMs 3000 -ErrorMessage "Ctrl+Y 未能重做修改" -Condition {
            $t = (T1_NormalizeEol (Get-EditorText -EditorElement $ed)).Trim()
            if ($t -eq "UPDATED_LINE") { return $t }
            return $null
        }
        Assert-Equal -Actual $afterRedo -Expected "UPDATED_LINE"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F9-05" -Tier 1 -Feature "F9" -Group "G4" -Title "修改模式支持 Tab 缩进输入及取消按钮/Esc 操作" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F9-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "tab_cancel.md"
        Write-BytesUtf8 -Path $doc -Text "LineOne"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T1_FindEditorControl -Window $session.MainWindow
        $edHwnd = [IntPtr]$ed.Current.NativeWindowHandle

        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $edHwnd -Vk 0x09 # Tab
        Start-Sleep -Milliseconds 120
        $txt = Get-EditorText -EditorElement $ed
        Assert-Match -Text $txt -Pattern "(\t|  )" -Message "Tab 键未能输入缩进制表符或空格"

        $cancelBtn = T1_FindCancelButton -Window $session.MainWindow
        Assert-True ($null -ne $cancelBtn) "缺少取消按钮"
        Invoke-Button -Element $cancelBtn -MainHwnd $session.Hwnd
        $dlg = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 3000
        Assert-True ($null -ne $dlg) "有 Tab 修改时点击取消应提示未保存确认"
        Invoke-ModalBucket -Modal $dlg -Bucket Discard
        $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "阅读模式\s*-\s*MDReader"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# F10: 保存原子写入磁盘并自动切回阅读模式 (T1-F10-01 .. T1-F10-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T1-F10-01" -Tier 1 -Feature "F10" -Group "G4" -Title "修改态点击'保存并阅读'原子写盘并自动切回阅读模式" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F10-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "save_btn.md"
        Write-BytesUtf8 -Path $doc -Text "# Before"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Invoke-Button -Element (T1_FindModeToggleButton -Window $session.MainWindow) -MainHwnd $session.Hwnd
        $ed = T1_FindEditorControl -Window $session.MainWindow
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text "# After Button Save" -ReplaceAll

        $saveBtn = T1_FindModeToggleButton -Window $session.MainWindow
        Assert-Equal -Actual $saveBtn.Current.Name -Expected "保存并阅读"
        Invoke-Button -Element $saveBtn -MainHwnd $session.Hwnd

        $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "^save_btn\.md\s*-\s*阅读模式\s*-\s*MDReader$"
        $meta = Read-FileBytesMeta -Path $doc
        Assert-Equal -Actual $meta.Text.Trim() -Expected "# After Button Save"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F10-02" -Tier 1 -Feature "F10" -Group "G4" -Title "修改态按 Ctrl+S 写回磁盘并自动切回阅读模式" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F10-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "ctrl_s.md"
        Write-BytesUtf8 -Path $doc -Text "# Old"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T1_FindEditorControl -Window $session.MainWindow
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text "# Saved Via Ctrl+S" -ReplaceAll
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$ed.Current.NativeWindowHandle) -Ctrl -Vk 0x53 # Ctrl+S

        $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "^ctrl_s\.md\s*-\s*阅读模式\s*-\s*MDReader$"
        Assert-True ($null -eq (T1_FindEditorControl -Window $session.MainWindow -TimeoutMs 300)) "保存后应自动隐藏编辑区"
        $meta = Read-FileBytesMeta -Path $doc
        Assert-Equal -Actual $meta.Text.Trim() -Expected "# Saved Via Ctrl+S"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F10-03" -Tier 1 -Feature "F10" -Group "G4" -Title "保存后磁盘文件与编辑内容逐字节一致" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F10-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "exact_bytes.md"
        Write-BytesUtf8 -Path $doc -Text "Old" -Bom $false -Eol 'LF'
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T1_FindEditorControl -Window $session.MainWindow
        $payload = "# Exact Byte Check`n第二行：中文标点，123。"
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text $payload -ReplaceAll
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$ed.Current.NativeWindowHandle) -Ctrl -Vk 0x53

        $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "阅读模式"
        $meta = Read-FileBytesMeta -Path $doc
        $expectedBytes = (New-Object System.Text.UTF8Encoding($false)).GetBytes($payload)
        Assert-Equal -Actual $meta.RawBytes.Length -Expected $expectedBytes.Length -Message "磁盘字节长度不一致"
        for ($i = 0; $i -lt $expectedBytes.Length; $i++) {
            if ($meta.RawBytes[$i] -ne $expectedBytes[$i]) {
                throw "字节索引 $i 不一致: 期望=$($expectedBytes[$i]), 实际=$($meta.RawBytes[$i])"
            }
        }
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F10-04" -Tier 1 -Feature "F10" -Group "G4" -Title "保存后清除 * 且阅读视图更新、再次切入编辑可见最新原文" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F10-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "reedit.md"
        Write-BytesUtf8 -Path $doc -Text "# Original"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        $shotBefore = Join-Path $ctx.SandboxDir "read_before.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotBefore

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T1_FindEditorControl -Window $session.MainWindow
        $newBody = "# Brand New Content Saved`n`nMore visible text lines added for reading diff verification."
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text $newBody -ReplaceAll
        $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern '\*'

        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$ed.Current.NativeWindowHandle) -Ctrl -Vk 0x53
        $cleanTitle = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "^reedit\.md\s*-\s*阅读模式\s*-\s*MDReader$"
        Assert-True ($cleanTitle -notmatch '\*') "保存后标题不应再包含 * 脏标记"

        Start-Sleep -Milliseconds 200
        $shotAfter = Join-Path $ctx.SandboxDir "read_after.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $shotAfter
        $diff = Compare-WindowCaptureDiff -PngPathA $shotBefore -PngPathB $shotAfter
        Assert-True ($diff -gt 0.002) "保存切回阅读模式后画面未反映更新内容"

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed2 = T1_FindEditorControl -Window $session.MainWindow
        Assert-Match -Text (T1_NormalizeEol (Get-EditorText -EditorElement $ed2)) -Pattern "Brand New Content Saved"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F10-05" -Tier 1 -Feature "F10" -Group "G4" -Title "保存后保留原文件换行风格 (CRLF/LF) 与 UTF-8 BOM" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F10-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $docCrlf = Join-Path $ctx.SandboxDir "crlf_nobom.md"
        Write-BytesUtf8 -Path $docCrlf -Text "Line1`nLine2" -Bom $false -Eol 'CRLF'
        $session = Start-AppInstance -Context $ctx -Arguments @($docCrlf) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T1_FindEditorControl -Window $session.MainWindow
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text "Line1`nLine2`nLine3" -ReplaceAll
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$ed.Current.NativeWindowHandle) -Ctrl -Vk 0x53
        $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "阅读模式"
        $meta1 = Read-FileBytesMeta -Path $docCrlf
        Assert-Equal -Actual $meta1.HasUtf8Bom -Expected $false -Message "无 BOM 文件保存后不应增加 BOM"
        Assert-Equal -Actual $meta1.LineEnding -Expected 'CRLF' -Message "CRLF 换行未保留"

        # 同一会话拖入带 UTF-8 BOM + LF 文件继续验证
        $docBomLf = Join-Path $ctx.SandboxDir "bom_lf.md"
        Write-BytesUtf8 -Path $docBomLf -Text "Alpha`nBeta" -Bom $true -Eol 'LF'
        Send-DropFiles -Hwnd $session.Hwnd -Paths @($docBomLf)
        $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "bom_lf\.md"

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed2 = T1_FindEditorControl -Window $session.MainWindow
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed2 -Text "Alpha`nBeta`nGamma" -ReplaceAll
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$ed2.Current.NativeWindowHandle) -Ctrl -Vk 0x53
        $null = T1_WaitTitleMatch -Window $session.MainWindow -Pattern "阅读模式"
        $meta2 = Read-FileBytesMeta -Path $docBomLf
        Assert-Equal -Actual $meta2.HasUtf8Bom -Expected $true -Message "UTF-8 BOM 未保留"
        Assert-Equal -Actual $meta2.LineEnding -Expected 'LF' -Message "LF 换行未保留"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# F11: 极简界面约束与单一本地 JSON 配置 (T1-F11-01 .. T1-F11-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T1-F11-01" -Tier 1 -Feature "F11" -Group "G4" -Title "界面零设置面板、零 Preferences/设置 选项卡或树控件" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F11-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "no_settings.md"
        Write-BytesUtf8 -Path $doc -Text "# Minimal UI"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        $all = $session.MainWindow.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
        foreach ($el in $all) {
            $ct = $el.Current.ControlType
            $name = [string]$el.Current.Name
            Assert-True ($ct -ne [System.Windows.Automation.ControlType]::Tab -and $ct -ne [System.Windows.Automation.ControlType]::Tree) "极简主窗口不应存在 Tab 或 Tree 控件"
            if ($name -match '^(设置|首选项|偏好设置|Settings|Preferences|Options)$') {
                throw "极简主窗口出现了禁止的设置类控件: Name='$name'"
            }
        }
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F11-02" -Tier 1 -Feature "F11" -Group "G4" -Title "主窗口可见工具栏操作按钮极简计数 (<= 4 个且无设置按钮)" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F11-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "btn_count.md"
        Write-BytesUtf8 -Path $doc -Text "# Button Count"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        [array]$btns = Get-ToolbarActionButtons -Root $session.MainWindow
        Assert-True ($btns.Count -ge 2 -and $btns.Count -le 4) "极简工具栏操作按钮数应为 2..4，实际=$($btns.Count)"
        foreach ($b in $btns) {
            $bName = [string]$b.Current.Name
            Assert-True ($bName -notmatch '设置|Settings|Preferences') "工具栏不应存在设置按钮: '$bName'"
        }
        $CaseEnv.Metrics.ToolbarButtonCount = $btns.Count
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F11-03" -Tier 1 -Feature "F11" -Group "G4" -Title "可执行文件同级目录 config.json 便携优先加载生效" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F11-03" -ExePath $CaseEnv.ExePath -CopyExeToSandbox -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "portable.md"
        $portableCfg = Join-Path $ctx.SandboxDir "config.json"
        Write-BytesUtf8 -Path $doc -Text "# Portable Config"
        Write-BytesUtf8 -Path $portableCfg -Text '{"theme": "dark"}'

        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        $lum = Wait-Condition -TimeoutMs 5000 -ErrorMessage "同级目录便携 config.json 未生效" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -ge 0 -and $l -le 95) { return $l }
            return $null
        }
        Assert-True ($lum -le 95)
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F11-04" -Tier 1 -Feature "F11" -Group "G4" -Title "--config 参数显式覆盖加载对应配置" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F11-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "cli_cfg.md"
        $cfg = Join-Path $ctx.SandboxDir "override.json"
        Write-BytesUtf8 -Path $doc -Text "# CLI Config"
        Write-BytesUtf8 -Path $cfg -Text '{"theme": "dark", "maxWidth": 840}'

        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs
        $lum = Wait-Condition -TimeoutMs 5000 -ErrorMessage "--config 配置未生效" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -ge 0 -and $l -le 95) { return $l }
            return $null
        }
        Assert-True ($lum -le 95)
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F11-05" -Tier 1 -Feature "F11" -Group "G4" -Title "修改 config.json 后通过 Ctrl+Shift+R 热重载及重启均生效" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F11-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    $session2 = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "reload.md"
        $cfg = Join-Path $ctx.SandboxDir "reload.json"
        Write-BytesUtf8 -Path $doc -Text "# Hot Reload"
        Write-BytesUtf8 -Path $cfg -Text '{"theme": "light"}'

        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs
        $lum1 = Wait-Condition -TimeoutMs 5000 -ErrorMessage "初始亮色未生效" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -ge 170) { return $l }
            return $null
        }

        Write-BytesUtf8 -Path $cfg -Text '{"theme": "dark"}'
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Shift -Vk 0x52 # Ctrl+Shift+R
        $lum2 = Wait-Condition -TimeoutMs 5000 -ErrorMessage "Ctrl+Shift+R 热重载暗色主题未生效" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -ge 0 -and $l -le 95) { return $l }
            return $null
        }
        Assert-True ($lum1 -gt $lum2 + 80) "热重载前后亮度未显著切换"
        Stop-AppInstance -Session $session
        $session = $null

        $session2 = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs
        $lum3 = Wait-Condition -TimeoutMs 5000 -ErrorMessage "重启后暗色配置未生效" -Condition {
            $l = Get-WindowLuminance -Hwnd $session2.Hwnd
            if ($l -ge 0 -and $l -le 95) { return $l }
            return $null
        }
        $CaseEnv.Metrics.HotReloadDiff = [math]::Round($lum1 - $lum2, 1)
        Assert-True ($lum3 -le 95)
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        if ($session2) { Stop-AppInstance -Session $session2 }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# F12: 极致内存控制与独立体验达标 (T1-F12-01 .. T1-F12-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T1-F12-01" -Tier 1 -Feature "F12" -Group "G4" -Title "空载冷启动 Working Set 内存基线达标且单进程无 WebView2" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F12-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $session = Start-AppInstance -Context $ctx -Arguments @() -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 1500
        $m = Get-ProcessTreeMetrics -Process $session.Process
        $CaseEnv.Metrics.WorkingSetMB = $m.WorkingSetMB
        $CaseEnv.Metrics.PrivateMemoryMB = $m.PrivateMemoryMB
        $CaseEnv.Metrics.ProcessCount = $m.ProcessCount

        Assert-Equal -Actual $m.ProcessCount -Expected 1 -Message "应为单进程架构"
        foreach ($cn in $m.ChildNames) {
            Assert-True ($cn -notmatch 'msedgewebview2|node') "禁止存在重型子进程: $cn"
        }
        if ($CaseEnv.IsSelfTest) {
            Assert-True ($m.WorkingSetMB -gt 0 -and $m.WorkingSetMB -lt 120) "SelfTest 替身内存异常: $($m.WorkingSetMB) MB"
        } else {
            Assert-True ($m.WorkingSetMB -gt 0 -and $m.WorkingSetMB -le 50) "真实应用空载冷启动 WorkingSet 超出 50MB 阈值: $($m.WorkingSetMB) MB"
        }
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F12-02" -Tier 1 -Feature "F12" -Group "G4" -Title "打开 100KB 典型 Markdown 文档 Working Set 保持轻量" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F12-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "typical_100k.md"
        $unit = "## Section Block`n`nThis is a typical paragraph with **bold**, *italic*, and ``code()`` tokens for memory evaluation.`n`n- Item A`n- Item B`n`n"
        $sb = New-Object System.Text.StringBuilder
        while ($sb.Length -lt 102400) { [void]$sb.Append($unit) }
        Write-BytesUtf8 -Path $doc -Text $sb.ToString()

        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 1500
        $m = Get-ProcessTreeMetrics -Process $session.Process
        $CaseEnv.Metrics.WorkingSetMB = $m.WorkingSetMB
        $CaseEnv.Metrics.ProcessCount = $m.ProcessCount

        Assert-Equal -Actual $m.ProcessCount -Expected 1 -Message "应为单进程架构"
        if ($CaseEnv.IsSelfTest) {
            Assert-True ($m.WorkingSetMB -gt 0 -and $m.WorkingSetMB -lt 130) "SelfTest 100KB 内存异常: $($m.WorkingSetMB) MB"
        } else {
            Assert-True ($m.WorkingSetMB -gt 0 -and $m.WorkingSetMB -le 60) "真实应用 100KB 文档 WorkingSet 超出 60MB 阈值: $($m.WorkingSetMB) MB"
        }
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F12-03" -Tier 1 -Feature "F12" -Group "G4" -Title "阅读正文视口面积占主窗口客户区面积 >= 85%" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F12-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "viewport.md"
        Write-BytesUtf8 -Path $doc -Text "# Viewport Ratio`n`nClean reading area check."
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        $cr = New-Object E2ENative+RECT
        Assert-True ([E2ENative]::GetClientRect($session.Hwnd, [ref]$cr)) "获取客户区矩形失败"
        $clientH = $cr.Bottom - $cr.Top
        Assert-True ($clientH -gt 100) "客户区高度异常: $clientH"

        [array]$btns = Get-ToolbarActionButtons -Root $session.MainWindow
        Assert-True ($btns.Count -ge 1) "未检测到工具栏按钮"
        $maxBottomClientY = 0
        foreach ($b in $btns) {
            $r = $b.Current.BoundingRectangle
            $pt = New-Object E2ENative+POINT
            $pt.X = [int]$r.Left
            $pt.Y = [int]$r.Bottom
            [void][E2ENative]::ScreenToClient($session.Hwnd, [ref]$pt)
            if ($pt.Y -gt $maxBottomClientY) { $maxBottomClientY = $pt.Y }
        }
        $toolbarHeight = [math]::Max(0, $maxBottomClientY + 6)
        $ratio = ($clientH - $toolbarHeight) / [double]$clientH
        $CaseEnv.Metrics.ViewportRatio = [math]::Round($ratio, 4)
        Assert-True ($ratio -ge 0.85) "阅读视口占比低于 85%: ratio=$ratio (clientH=$clientH, toolbarH=$toolbarHeight)"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F12-04" -Tier 1 -Feature "F12" -Group "G4" -Title "多元素全量 GFM 文档渲染无卡死且无异常退出" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F12-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "sample-gfm-full.md"
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "sample-gfm-full.md") -Destination $doc
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "test-image.png") -Destination (Join-Path $ctx.SandboxDir "test-image.png")

        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 1000
        Assert-True (-not $session.Process.HasExited) "多元素文档渲染后进程意外退出"
        Assert-True ($null -eq (Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 300)) "不应出现错误模态框"
        T1_AssertClientViewportClean -Hwnd $session.Hwnd -WorkDir $ctx.SandboxDir
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T1-F12-05" -Tier 1 -Feature "F12" -Group "G4" -Title "采集并输出完整结构化进程内存与句柄度量指标" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T1-F12-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "metrics.md"
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "sample-notes.md") -Destination $doc
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 800

        $m = Get-ProcessTreeMetrics -Process $session.Process
        [array]$btns = Get-ToolbarActionButtons -Root $session.MainWindow
        $CaseEnv.Metrics.WorkingSetMB = $m.WorkingSetMB
        $CaseEnv.Metrics.PrivateMemoryMB = $m.PrivateMemoryMB
        $CaseEnv.Metrics.HandleCount = $m.HandleCount
        $CaseEnv.Metrics.ProcessCount = $m.ProcessCount
        $CaseEnv.Metrics.ToolbarButtonCount = $btns.Count

        Assert-True ($m.WorkingSetMB -gt 0) "WorkingSetMB 无效"
        Assert-True ($m.PrivateMemoryMB -gt 0) "PrivateMemoryMB 无效"
        Assert-True ($m.HandleCount -gt 0) "HandleCount 无效"
        Assert-Equal -Actual $m.ProcessCount -Expected 1
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

return $cases.ToArray()
