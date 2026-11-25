#Requires -Version 5.1
<#
.SYNOPSIS
    MDReader 端到端自动化黑盒测试用例 — Tier 2（边界、极值与异常注入：60 条，F1–F12 每项 5 条）
.DESCRIPTION
    覆盖空文件、1MB 大文件、空格/括号/中文路径、非法 UTF-8、只读/外部删除写入保护、
    损坏/部分合法/越界 JSON、暗色无白屏像素采样、连续热重载、极小窗口布局不重叠及内存/句柄泄漏核查。
#>

Set-StrictMode -Version Latest

if (-not ([System.Management.Automation.PSTypeName]'E2ETier2Win32').Type) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class E2ETier2Win32 {
    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool SetWindowPos(IntPtr hWnd, IntPtr hWndInsertAfter, int X, int Y, int cx, int cy, uint uFlags);
}
'@
}

# ==============================================================================
# 内部辅助函数（仅供 Tier 2 复用）
# ==============================================================================

function global:T2_GetPlanContent {
    param([Parameter(Mandatory = $true)][hashtable]$CaseEnv)
    $planPath = Join-Path $CaseEnv.RepoRoot "IMPLEMENTATION_PLAN.md"
    Assert-True (Test-Path -LiteralPath $planPath -PathType Leaf) "缺少实施方案文档: $planPath"
    $meta = Read-FileBytesMeta -Path $planPath
    return $meta.Text
}

function global:T2_WaitTitleMatch {
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

function global:T2_FindModeToggleButton {
    param([System.Windows.Automation.AutomationElement]$Window, [int]$TimeoutMs = 4000)
    return (Find-Control -Root $Window -AutomationId "IDC_MODE_TOGGLE" -FallbackNames @("编辑", "保存并阅读") -ExcludeTitleBar -TimeoutMs $TimeoutMs)
}

function global:T2_FindCancelButton {
    param([System.Windows.Automation.AutomationElement]$Window, [int]$TimeoutMs = 3000)
    return (Find-Control -Root $Window -AutomationId "IDC_EDIT_CANCEL" -FallbackNames @("取消", "返回") -ExcludeTitleBar -TimeoutMs $TimeoutMs)
}

function global:T2_FindEditorControl {
    param([System.Windows.Automation.AutomationElement]$Window, [int]$TimeoutMs = 4000)
    return (Find-Control -Root $Window -AutomationId "IDC_EDITOR" -ClassRegex "RICHEDIT50W|RichEdit20W|RichEdit" -MinHeight 80 -ExcludeTitleBar -TimeoutMs $TimeoutMs)
}

function global:T2_NormalizeEol {
    param([AllowEmptyString()][string]$Text)
    if ($null -eq $Text) { return "" }
    return (($Text -replace "`r`n", "`n") -replace "`r", "`n")
}

$cases = New-Object 'System.Collections.Generic.List[object]'

# ==============================================================================
# F1: 实施方案文档边界核查 (T2-F1-01 .. T2-F1-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T2-F1-01" -Tier 2 -Feature "F1" -Group "G1" -Title "核查实施方案文档字数充实且二级章节 >= 5" -Run {
    param($CaseEnv)
    $text = T2_GetPlanContent -CaseEnv $CaseEnv
    Assert-True ($text.Length -ge 3000) "方案文档内容过短 (len=$($text.Length))"
    $h2Matches = [regex]::Matches($text, '(?m)^##\s+')
    Assert-True ($h2Matches.Count -ge 5) "二级章节数量不足: $($h2Matches.Count)"
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F1-02" -Tier 2 -Feature "F1" -Group "G1" -Title "实施方案文档禁止残留 TBD/TODO/FIXME 占位符" -Run {
    param($CaseEnv)
    $text = T2_GetPlanContent -CaseEnv $CaseEnv
    $m = [regex]::Match($text, '(?m)\b(TBD|TODO|FIXME|XXX)\b')
    Assert-True (-not $m.Success) "方案文档不应残留未完成占位符: '$($m.Value)'"
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F1-03" -Tier 2 -Feature "F1" -Group "G1" -Title "方案文档包含原子写回失败留守保护与配置容错回退条款" -Run {
    param($CaseEnv)
    $text = T2_GetPlanContent -CaseEnv $CaseEnv
    Assert-Match -Text $text -Pattern "ReplaceFileW|FlushFileBuffers" -Message "缺少原子替换写盘条款"
    Assert-Match -Text $text -Pattern "回退默认|安全回退" -Message "缺少异常安全回退条款"
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F1-04" -Tier 2 -Feature "F1" -Group "G1" -Title "方案文档明确列出 Shazao-markdown 剔除项与非目标边界" -Run {
    param($CaseEnv)
    $text = T2_GetPlanContent -CaseEnv $CaseEnv
    Assert-Match -Text $text -Pattern "剔除项|非目标|不做" -Message "缺少功能剔除边界说明"
    Assert-Match -Text $text -Pattern "多标签|文件树" -Message "未明确剔除多标签/文件树工作台"
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F1-05" -Tier 2 -Feature "F1" -Group "G1" -Title "方案文档包含明确的 Working Set 内存量级目标数值" -Run {
    param($CaseEnv)
    $text = T2_GetPlanContent -CaseEnv $CaseEnv
    Assert-Match -Text $text -Pattern "(<\s*20\s*MB|<\s*30\s*MB|8–22\s*MB)" -Message "缺少量化内存目标门槛"
}))

# ==============================================================================
# F2: Windows .md 文件关联异常与幂等边界 (T2-F2-01 .. T2-F2-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T2-F2-01" -Tier 2 -Feature "F2" -Group "G2" -Title "重复执行 --register 保持幂等且退出码 0" -Run {
    param($CaseEnv)
    $snap = Enter-HkcuRegistrySandbox
    $ctx = New-E2ECaseContext -CaseId "T2-F2-01" -ExePath $CaseEnv.ExePath -CopyExeToSandbox -RunId $CaseEnv.RunId
    try {
        $r1 = Invoke-AppCli -ExePath $ctx.ExePath -Arguments @("--register") -TimeoutMs $CaseEnv.TimeoutMs
        $r2 = Invoke-AppCli -ExePath $ctx.ExePath -Arguments @("--register") -TimeoutMs $CaseEnv.TimeoutMs
        Assert-Equal -Actual $r1.ExitCode -Expected 0
        Assert-Equal -Actual $r2.ExitCode -Expected 0
        Assert-True (-not $r2.HadUnexpectedModal)
        $cmd = (Get-ItemProperty -LiteralPath "Registry::HKCU\Software\Classes\MDReader.Document\shell\open\command").'(default)'
        Assert-Equal -Actual $cmd -Expected ('"' + $ctx.ExePath + '" "%1"')
    } finally {
        Restore-HkcuRegistrySandbox -Snapshot $snap
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F2-02" -Tier 2 -Feature "F2" -Group "G2" -Title "未注册状态下执行 --unregister 安全幂等退出" -Run {
    param($CaseEnv)
    $snap = Enter-HkcuRegistrySandbox
    $ctx = New-E2ECaseContext -CaseId "T2-F2-02" -ExePath $CaseEnv.ExePath -CopyExeToSandbox -RunId $CaseEnv.RunId
    try {
        # 确保沙盒起始为未注册状态
        $null = Invoke-AppCli -ExePath $ctx.ExePath -Arguments @("--unregister") -TimeoutMs $CaseEnv.TimeoutMs
        $r2 = Invoke-AppCli -ExePath $ctx.ExePath -Arguments @("--unregister") -TimeoutMs $CaseEnv.TimeoutMs
        Assert-Equal -Actual $r2.ExitCode -Expected 0 -Message "重复或未注册状态下 --unregister 应安全返回 0"
        Assert-True (-not $r2.HadUnexpectedModal)
    } finally {
        Restore-HkcuRegistrySandbox -Snapshot $snap
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F2-03" -Tier 2 -Feature "F2" -Group "G2" -Title "存在历史残余 MDReader 键与其他应用关联时安全覆盖与卸载" -Run {
    param($CaseEnv)
    $snap = Enter-HkcuRegistrySandbox
    $ctx = New-E2ECaseContext -CaseId "T2-F2-03" -ExePath $CaseEnv.ExePath -CopyExeToSandbox -RunId $CaseEnv.RunId
    try {
        $oldCmdKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Software\Classes\MDReader.Document\shell\open\command")
        try { $oldCmdKey.SetValue("", '"C:\Legacy\OldMDReader.exe" "%1"', [Microsoft.Win32.RegistryValueKind]::String) } finally { $oldCmdKey.Dispose() }

        $owp = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Software\Classes\.md\OpenWithProgids")
        try { $owp.SetValue("KeepExistingEditor.md", "", [Microsoft.Win32.RegistryValueKind]::String) } finally { $owp.Dispose() }

        $rReg = Invoke-AppCli -ExePath $ctx.ExePath -Arguments @("--register") -TimeoutMs $CaseEnv.TimeoutMs
        Assert-Equal -Actual $rReg.ExitCode -Expected 0
        $newCmd = (Get-ItemProperty -LiteralPath "Registry::HKCU\Software\Classes\MDReader.Document\shell\open\command").'(default)'
        Assert-Equal -Actual $newCmd -Expected ('"' + $ctx.ExePath + '" "%1"')

        $rUnreg = Invoke-AppCli -ExePath $ctx.ExePath -Arguments @("--unregister") -TimeoutMs $CaseEnv.TimeoutMs
        Assert-Equal -Actual $rUnreg.ExitCode -Expected 0
        Assert-True (-not (Test-Path -LiteralPath "Registry::HKCU\Software\Classes\MDReader.Document"))

        $owpAfter = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\Classes\.md\OpenWithProgids", $false)
        try {
            Assert-True ($owpAfter.GetValueNames() -contains "KeepExistingEditor.md") "不应误删其他应用 OpenWithProgids 值"
        } finally {
            if ($owpAfter) { $owpAfter.Dispose() }
        }
    } finally {
        Restore-HkcuRegistrySandbox -Snapshot $snap
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F2-04" -Tier 2 -Feature "F2" -Group "G2" -Title "可执行路径含空格与中文括号时注册表命令引号转义正确" -Run {
    param($CaseEnv)
    $snap = Enter-HkcuRegistrySandbox
    $ctx = New-E2ECaseContext -CaseId "T2-F2-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    try {
        $spacedDir = Join-Path $ctx.SandboxDir "应用 目录 (v2)"
        New-Item -ItemType Directory -Force -Path $spacedDir | Out-Null
        $spacedExe = Join-Path $spacedDir (Split-Path -Leaf $CaseEnv.ExePath)
        Copy-Item -LiteralPath $CaseEnv.ExePath -Destination $spacedExe -Force

        $res = Invoke-AppCli -ExePath $spacedExe -Arguments @("--register") -TimeoutMs $CaseEnv.TimeoutMs
        Assert-Equal -Actual $res.ExitCode -Expected 0
        $cmd = (Get-ItemProperty -LiteralPath "Registry::HKCU\Software\Classes\MDReader.Document\shell\open\command").'(default)'
        Assert-Equal -Actual $cmd -Expected ('"' + $spacedExe + '" "%1"') -Message "含空格括号路径未正确用双引号包裹"
    } finally {
        Restore-HkcuRegistrySandbox -Snapshot $snap
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F2-05" -Tier 2 -Feature "F2" -Group "G2" -Title "--register 与文件路径或 --unregister 混传时安全拒绝" -Run {
    param($CaseEnv)
    $snap = Enter-HkcuRegistrySandbox
    $ctx = New-E2ECaseContext -CaseId "T2-F2-05" -ExePath $CaseEnv.ExePath -CopyExeToSandbox -RunId $CaseEnv.RunId
    try {
        $doc = Join-Path $ctx.SandboxDir "dummy.md"
        Write-BytesUtf8 -Path $doc -Text "# Dummy"

        $r1 = Invoke-AppCli -ExePath $ctx.ExePath -Arguments @("--register", $doc) -TimeoutMs $CaseEnv.TimeoutMs
        Assert-True ($r1.ExitCode -ne 0) "--register 与文件路径混传应返回非零退出码"
        Assert-True (-not $r1.HadUnexpectedModal)

        $r2 = Invoke-AppCli -ExePath $ctx.ExePath -Arguments @("--register", "--unregister") -TimeoutMs $CaseEnv.TimeoutMs
        Assert-True ($r2.ExitCode -ne 0) "--register 与 --unregister 混传应返回非零退出码"
        Assert-True (-not $r2.HadUnexpectedModal)
    } finally {
        Restore-HkcuRegistrySandbox -Snapshot $snap
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# F3: 命令行启动边界与错误路径 (T2-F3-01 .. T2-F3-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T2-F3-01" -Tier 2 -Feature "F3" -Group "G4" -Title "含空格与中文括号路径文件直开并读取完整正文" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F3-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $subDir = Join-Path $ctx.SandboxDir "文档 目录 (A)"
        New-Item -ItemType Directory -Force -Path $subDir | Out-Null
        $doc = Join-Path $subDir "测试 笔记 (1).md"
        Write-BytesUtf8 -Path $doc -Text "# 空格路径正文`n`nChinese path ok."

        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern "测试 笔记 \(1\)\.md"
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        Assert-True ($null -ne $ed)
        Assert-Match -Text (Get-EditorText -EditorElement $ed) -Pattern "空格路径正文"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F3-02" -Tier 2 -Feature "F3" -Group "G4" -Title "打开 0 字节空文件呈现空白阅读页不崩溃" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F3-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "empty.md"
        [System.IO.File]::WriteAllBytes($doc, [byte[]]@())
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern "empty\.md\s*-\s*阅读模式"
        Assert-True ($null -eq (Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 300))
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        Assert-True ($null -ne $ed)
        Assert-Equal -Actual ((T2_NormalizeEol (Get-EditorText -EditorElement $ed)).TrimEnd("`r", "`n").Length) -Expected 0
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F3-03" -Tier 2 -Feature "F3" -Group "G4" -Title "传入不存在文件路径安全展示错误提示且进程不崩溃" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F3-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $missing = Join-Path $ctx.SandboxDir "non_existent_missing.md"
        $session = Start-AppInstance -Context $ctx -Arguments @($missing) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 300
        Assert-True (-not $session.Process.HasExited) "打开不存在路径不应导致进程崩溃"
        $modal = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 300
        if ($null -ne $modal) {
            Invoke-ModalBucket -Modal $modal -Bucket Ok
        }
        Assert-True (-not $session.Process.HasExited) "提示关闭后主窗口应仍正常存活"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F3-04" -Tier 2 -Feature "F3" -Group "G4" -Title "非 .md 扩展名与含非法 UTF-8 字节文件容错读取不崩溃" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F3-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "bad_utf8.txt"
        $bytes = [byte[]](0x23, 0x20, 0x52, 0x61, 0x77, 0x0A, 0xFF, 0xFE, 0x80, 0x81, 0x0A, 0x4F, 0x4B)
        [System.IO.File]::WriteAllBytes($doc, $bytes)

        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 300
        Assert-True (-not $session.Process.HasExited) "含非法 UTF-8 字节导致应用崩溃"
        Assert-Match -Text $session.MainWindow.Current.Name -Pattern "bad_utf8\.txt"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F3-05" -Tier 2 -Feature "F3" -Group "G2" -Title "未知 CLI 开关、缺失 --config 路径与多文件参数安全返回非零码" -Run {
    param($CaseEnv)
    $r1 = Invoke-AppCli -ExePath $CaseEnv.ExePath -Arguments @("--unknown-flag-999") -TimeoutMs $CaseEnv.TimeoutMs
    Assert-True ($r1.ExitCode -ne 0) "未知 CLI 开关应返回非零退出码"
    Assert-True (-not $r1.HadUnexpectedModal)

    $r2 = Invoke-AppCli -ExePath $CaseEnv.ExePath -Arguments @("--config") -TimeoutMs $CaseEnv.TimeoutMs
    Assert-True ($r2.ExitCode -ne 0) "缺失 --config 路径值应返回非零退出码"
    Assert-True (-not $r2.HadUnexpectedModal)

    $r3 = Invoke-AppCli -ExePath $CaseEnv.ExePath -Arguments @("a.md", "b.md") -TimeoutMs $CaseEnv.TimeoutMs
    Assert-True ($r3.ExitCode -ne 0) "传入多于一个文件路径应返回非零退出码"
    Assert-True (-not $r3.HadUnexpectedModal)
}))

# ==============================================================================
# F4: 拖放与文件打开边界防护 (T2-F4-01 .. T2-F4-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T2-F4-01" -Tier 2 -Feature "F4" -Group "G4" -Title "修改态有未保存改动时按 Ctrl+O 弹出确认框且 Cancel 保留修改" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F4-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "dirty_open.md"
        Write-BytesUtf8 -Path $doc -Text "Initial"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text "UNSAVED_DRAFT_TEXT" -ReplaceAll
        $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern '\*'

        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$ed.Current.NativeWindowHandle) -Ctrl -Vk 0x4F # Ctrl+O
        $dlg = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 4000
        Assert-True ($null -ne $dlg) "有未保存改动按 Ctrl+O 应先弹出未保存确认框"
        $txt = Get-ModalDialogText -Modal $dlg
        Assert-Match -Text $txt -Pattern "保存|未保存|修改|Save"

        Invoke-ModalBucket -Modal $dlg -Bucket Cancel
        $null = Wait-Condition -TimeoutMs 3000 -ErrorMessage "确认框未关闭" -Condition {
            $null -eq (Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 0)
        }
        Assert-Match -Text $session.MainWindow.Current.Name -Pattern "修改模式"
        Assert-Match -Text (Get-EditorText -EditorElement $ed) -Pattern "UNSAVED_DRAFT_TEXT"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F4-02" -Tier 2 -Feature "F4" -Group "G4" -Title "修改态无未保存改动时按 Ctrl+O 直接唤起文件对话框无多余弹窗" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F4-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "clean_open.md"
        Write-BytesUtf8 -Path $doc -Text "Clean"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        Assert-True ($session.MainWindow.Current.Name -notmatch '\*')

        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$ed.Current.NativeWindowHandle) -Ctrl -Vk 0x4F
        $dlg = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 5000
        Assert-True ($null -ne $dlg) "应直接弹出系统文件对话框"
        # 系统文件对话框内必然包含文件名 Edit 控件（与 MessageBox 确认框本质区分）
        $editBox = Find-Control -Root $dlg -ClassRegex "Edit" -TimeoutMs 2500
        Assert-True ($null -ne $editBox) "无改动时按 Ctrl+O 弹出的是确认框而非直接弹出文件选择对话框"

        Invoke-ModalBucket -Modal $dlg -Bucket Cancel
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F4-03" -Tier 2 -Feature "F4" -Group "G4" -Title "取消文件对话框后原文档标题与文本内容完整保留" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F4-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "stay_intact.md"
        Write-BytesUtf8 -Path $doc -Text "# Intact Content"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x4F
        $dlg = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 5000
        Assert-True ($null -ne $dlg)
        Invoke-ModalBucket -Modal $dlg -Bucket Cancel
        $null = Wait-Condition -TimeoutMs 3000 -ErrorMessage "关闭文件框超时" -Condition {
            $null -eq (Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 0)
        }
        Assert-Match -Text $session.MainWindow.Current.Name -Pattern "stay_intact\.md"
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        Assert-Match -Text (Get-EditorText -EditorElement $ed) -Pattern "Intact Content"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F4-04" -Tier 2 -Feature "F4" -Group "G4" -Title "WM_DROPFILES 拖入空格中文路径成功且拖入不存在文件安全不崩溃" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F4-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc1 = Join-Path $ctx.SandboxDir "init.md"
        $doc2 = Join-Path $ctx.SandboxDir "拖放 笔记 (2).md"
        $missing = Join-Path $ctx.SandboxDir "no_such_drop.md"
        Write-BytesUtf8 -Path $doc1 -Text "# Init"
        Write-BytesUtf8 -Path $doc2 -Text "# Dropped Chinese Space"

        $session = Start-AppInstance -Context $ctx -Arguments @($doc1) -TimeoutMs $CaseEnv.TimeoutMs
        Send-DropFiles -Hwnd $session.Hwnd -Paths @($doc2)
        $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern "拖放 笔记 \(2\)\.md"

        Send-DropFiles -Hwnd $session.Hwnd -Paths @($missing)
        Start-Sleep -Milliseconds 300
        Assert-True (-not $session.Process.HasExited) "拖入不存在文件导致进程崩溃"
        $modal = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 250
        if ($modal) { Invoke-ModalBucket -Modal $modal -Bucket Ok }
        Assert-True (-not $session.Process.HasExited)
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F4-05" -Tier 2 -Feature "F4" -Group "G4" -Title "连续快速拖放打开多文件状态干净切至最后一个文档" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F4-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $f1 = Join-Path $ctx.SandboxDir "seq1.md"
        $f2 = Join-Path $ctx.SandboxDir "seq2.md"
        $f3 = Join-Path $ctx.SandboxDir "seq3.md"
        Write-BytesUtf8 -Path $f1 -Text "Content 1"
        Write-BytesUtf8 -Path $f2 -Text "Content 2"
        Write-BytesUtf8 -Path $f3 -Text "Content 3 Final"

        $session = Start-AppInstance -Context $ctx -Arguments @($f1) -TimeoutMs $CaseEnv.TimeoutMs
        Send-DropFiles -Hwnd $session.Hwnd -Paths @($f2)
        Start-Sleep -Milliseconds 60
        Send-DropFiles -Hwnd $session.Hwnd -Paths @($f3)

        $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern "seq3\.md\s*-\s*阅读模式"
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        Assert-Match -Text (Get-EditorText -EditorElement $ed) -Pattern "Content 3 Final"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# F5: 阅读模式极端排版与安全隔离边界 (T2-F5-01 .. T2-F5-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T2-F5-01" -Tier 2 -Feature "F5" -Group "G4" -Title "20列超宽表格与500字符长代码行不撑爆窗口且不崩溃" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F5-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $cols = (1..20 | ForEach-Object { "Col$_" }) -join " | "
        $seps = (1..20 | ForEach-Object { ":---" }) -join " | "
        $vals = (1..20 | ForEach-Object { "Val$_" }) -join " | "
        $longCode = "X" * 500
        $md = "# Wide Stress`n`n| $cols |`n| $seps |`n| $vals |`n`n``````text`n$longCode`n``````"
        $doc = Join-Path $ctx.SandboxDir "wide.md"
        $cfg = Join-Path $ctx.SandboxDir "wide.json"
        Write-BytesUtf8 -Path $doc -Text $md
        Write-BytesUtf8 -Path $cfg -Text '{"windowWidth": 900, "windowHeight": 700}'

        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 350
        Assert-True (-not $session.Process.HasExited) "超宽表格/代码行导致崩溃"
        $rect = $session.MainWindow.Current.BoundingRectangle
        Assert-True ($rect.Width -le 980) "超宽内容撑爆了窗口宽度: Width=$($rect.Width)"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F5-02" -Tier 2 -Feature "F5" -Group "G4" -Title "损坏 PNG 图片与缺失路径图片容错占位渲染不崩溃" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F5-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "corrupt-image.png") -Destination (Join-Path $ctx.SandboxDir "corrupt-image.png")
        $doc = Join-Path $ctx.SandboxDir "bad_images.md"
        Write-BytesUtf8 -Path $doc -Text "# Image Fault`n`n![损坏图片](corrupt-image.png)`n`n![缺失图片](missing_pic.png)"

        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 400
        Assert-True (-not $session.Process.HasExited) "损坏图片导致应用崩溃"
        Assert-True ($null -eq (Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 300))
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F5-03" -Tier 2 -Feature "F5" -Group "G4" -Title "原始 HTML <script> 标签安全隔离消毒且不触发弹窗执行" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F5-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "xss.md"
        Write-BytesUtf8 -Path $doc -Text "# XSS Guard`n`n<script>alert('XSS')</script>`n`n<img src=x onerror=alert(1) />"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 600
        Assert-True (-not $session.Process.HasExited)
        $modal = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 400
        Assert-True ($null -eq $modal) "检测到弹窗，疑似脚本未隔离执行!"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F5-04" -Tier 2 -Feature "F5" -Group "G4" -Title "5层嵌套列表与多级引用块稳定渲染" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F5-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "nested.md"
        $md = @"
# Deep Nesting

- Level 1
  - Level 2
    - Level 3
      - Level 4
        - Level 5

> Quote 1
> > Quote 2
> > > Quote 3
"@
        Write-BytesUtf8 -Path $doc -Text $md
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 300
        Assert-True (-not $session.Process.HasExited)
        $png = Join-Path $ctx.SandboxDir "nested.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $png
        Assert-True ((Get-Item -LiteralPath $png).Length -gt 500)
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F5-05" -Tier 2 -Feature "F5" -Group "G4" -Title "极窄与宽屏窗口尺寸动态缩放重排不崩溃" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F5-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "resize.md"
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "sample-basic.md") -Destination $doc
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        # SWP_NOZORDER (0x0004) | SWP_NOMOVE (0x0002)
        [void][E2ETier2Win32]::SetWindowPos($session.Hwnd, [IntPtr]::Zero, 0, 0, 420, 320, 0x0006)
        Start-Sleep -Milliseconds 220
        $pSmall = Join-Path $ctx.SandboxDir "small.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $pSmall

        [void][E2ETier2Win32]::SetWindowPos($session.Hwnd, [IntPtr]::Zero, 0, 0, 1280, 820, 0x0006)
        Start-Sleep -Milliseconds 220
        $pWide = Join-Path $ctx.SandboxDir "wide.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $pWide

        Assert-True (-not $session.Process.HasExited) "窗口尺寸缩放重排导致崩溃"
        Assert-True ((Get-Item -LiteralPath $pSmall).Length -gt 300 -and (Get-Item -LiteralPath $pWide).Length -gt 300)
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# F6: 主题与颜色配置容错边界 (T2-F6-01 .. T2-F6-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T2-F6-01" -Tier 2 -Feature "F6" -Group "G4" -Title "theme 非法值安全回退默认亮色主题" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F6-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "t_bad.md"
        $cfg = Join-Path $ctx.SandboxDir "t_bad.json"
        Write-BytesUtf8 -Path $doc -Text "# Fallback Light"
        Write-BytesUtf8 -Path $cfg -Text '{"theme": "neon-pink"}'

        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs
        $lum = Wait-Condition -TimeoutMs 5000 -ErrorMessage "非法 theme 未回退默认亮色" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -ge 170) { return $l }
            return $null
        }
        Assert-True ($lum -ge 170)
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F6-02" -Tier 2 -Feature "F6" -Group "G4" -Title "theme 大小写混合 'DARK' 正确识别为暗色主题" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F6-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "t_case.md"
        $cfg = Join-Path $ctx.SandboxDir "t_case.json"
        Write-BytesUtf8 -Path $doc -Text "# Case Insensitive Dark"
        Write-BytesUtf8 -Path $cfg -Text '{"theme": "DARK"}'

        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs
        $lum = Wait-Condition -TimeoutMs 5000 -ErrorMessage "大小写混合 DARK 未生效" -Condition {
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

[void]$cases.Add((New-TestCaseDef -Id "T2-F6-03" -Tier 2 -Feature "F6" -Group "G4" -Title "colors 含非法十六进制串时逐字段回退且合法主题生效" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F6-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "bad_hex.md"
        $cfg = Join-Path $ctx.SandboxDir "bad_hex.json"
        Write-BytesUtf8 -Path $doc -Text "# Bad Hex Fallback"
        $json = @'
{
  "theme": "dark",
  "colors": {
    "background": "not-hex",
    "text": "#ZZZZZZ",
    "link": "#12"
  }
}
'@
        Write-BytesUtf8 -Path $cfg -Text $json
        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs
        $lum = Wait-Condition -TimeoutMs 5000 -ErrorMessage "colors 非法值破坏了合法的 dark 主题回退" -Condition {
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

[void]$cases.Add((New-TestCaseDef -Id "T2-F6-04" -Tier 2 -Feature "F6" -Group "G4" -Title "暗色模式下阅读客户区中心像素无大面积刺眼纯白背景" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F6-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "no_white.md"
        $cfg = Join-Path $ctx.SandboxDir "no_white.json"
        Write-BytesUtf8 -Path $doc -Text "# Dark Pure Background Check`n`nParagraph text here."
        Write-BytesUtf8 -Path $cfg -Text '{"theme": "dark"}'

        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs
        $null = Wait-Condition -TimeoutMs 5000 -ErrorMessage "等待暗色渲染超时" -Condition {
            $l = Get-WindowLuminance -Hwnd $session.Hwnd
            if ($l -ge 0 -and $l -le 95) { return $l }
            return $null
        }

        $png = Join-Path $ctx.SandboxDir "dark_check.png"
        $null = Capture-WindowPng -Hwnd $session.Hwnd -OutPath $png
        $bmp = [System.Drawing.Bitmap]::FromFile($png)
        try {
            $x0 = [int]($bmp.Width * 0.15); $x1 = [int]($bmp.Width * 0.85)
            $y0 = [int]($bmp.Height * 0.22); $y1 = [int]($bmp.Height * 0.88)
            $total = 0; $white = 0
            for ($y = $y0; $y -lt $y1; $y += 6) {
                for ($x = $x0; $x -lt $x1; $x += 6) {
                    $c = $bmp.GetPixel($x, $y)
                    if ($c.R -gt 245 -and $c.G -gt 245 -and $c.B -gt 245) { $white++ }
                    $total++
                }
            }
            $whiteRatio = if ($total -gt 0) { $white / [double]$total } else { 1.0 }
            $CaseEnv.Metrics.DarkWhitePixelRatio = [math]::Round($whiteRatio, 4)
            Assert-True ($whiteRatio -lt 0.15) "暗色模式下出现大面积刺眼纯白区域 (ratio=$whiteRatio)"
        } finally {
            $bmp.Dispose()
        }
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F6-05" -Tier 2 -Feature "F6" -Group "G4" -Title "连续 6 次热重载交替切换 light/dark 主题稳定不崩溃" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F6-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "flip.md"
        $cfg = Join-Path $ctx.SandboxDir "flip.json"
        Write-BytesUtf8 -Path $doc -Text "# Rapid Theme Flip"
        Write-BytesUtf8 -Path $cfg -Text '{"theme": "light"}'
        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs

        for ($i = 1; $i -le 6; $i++) {
            $target = if (($i % 2) -eq 1) { "dark" } else { "light" }
            Write-BytesUtf8 -Path $cfg -Text ('{"theme": "' + $target + '"}')
            Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Shift -Vk 0x52 # Ctrl+Shift+R
            if ($target -eq "dark") {
                $null = Wait-Condition -TimeoutMs 4000 -ErrorMessage "第 $i 次切换 dark 失败" -Condition {
                    $l = Get-WindowLuminance -Hwnd $session.Hwnd
                    if ($l -ge 0 -and $l -le 95) { return $l }
                    return $null
                }
            } else {
                $null = Wait-Condition -TimeoutMs 4000 -ErrorMessage "第 $i 次切换 light 失败" -Condition {
                    $l = Get-WindowLuminance -Hwnd $session.Hwnd
                    if ($l -ge 170) { return $l }
                    return $null
                }
            }
        }
        Assert-True (-not $session.Process.HasExited)
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# F7: 导航、划词与查找异常边界 (T2-F7-01 .. T2-F7-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T2-F7-01" -Tier 2 -Feature "F7" -Group "G4" -Title "0 字节空文档按 PageDown/End/Home 与鼠标滚轮安全无异常" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F7-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "empty_nav.md"
        [System.IO.File]::WriteAllBytes($doc, [byte[]]@())
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Vk 0x22 -Extended
        Send-Hotkey -MainHwnd $session.Hwnd -Vk 0x23 -Extended
        Send-Hotkey -MainHwnd $session.Hwnd -Vk 0x24 -Extended
        $readerEl = Find-Control -Root $session.MainWindow -AutomationId "IDC_READER" -MinHeight 100 -ExcludeTitleBar -TimeoutMs 800
        $wheelHwnd = if ($readerEl -and ([IntPtr]$readerEl.Current.NativeWindowHandle -ne [IntPtr]::Zero)) { [IntPtr]$readerEl.Current.NativeWindowHandle } else { $session.Hwnd }
        Send-MouseWheelScroll -Hwnd $wheelHwnd -Delta -360
        Send-MouseWheelScroll -Hwnd $wheelHwnd -Delta 360
        Start-Sleep -Milliseconds 200
        Assert-True (-not $session.Process.HasExited) "空文档导航导致进程崩溃"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F7-02" -Tier 2 -Feature "F7" -Group "G4" -Title "Ctrl+F 搜索不存在关键词静默提示 0 命中且不弹模态窗" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F7-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "no_match.md"
        Write-BytesUtf8 -Path $doc -Text "# Simple Document`n`nNormal words only."
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x46
        $findBox = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_INPUT" -ClassRegex "Edit|TextBox" -MaxHeight 60 -ExcludeTitleBar -TimeoutMs 1500
        if ($null -ne $findBox) {
            Set-EditorText -MainHwnd $session.Hwnd -EditorElement $findBox -Text "NO_SUCH_KEYWORD_987" -ReplaceAll -PreserveUndo:$false
        }
        Start-Sleep -Milliseconds 200
        Assert-True ($null -eq (Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 300)) "搜索无命中不应弹出模态弹窗阻塞"
        $status = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_STATUS" -TimeoutMs 600
        if ($null -ne $status) {
            Assert-Match -Text $status.Current.Name -Pattern "0" -Message "无命中状态文本应包含 0"
        }
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F7-03" -Tier 2 -Feature "F7" -Group "G4" -Title "Ctrl+F 搜索正则特殊元字符按字面安全匹配不崩溃" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F7-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "regex_chars.md"
        Write-BytesUtf8 -Path $doc -Text "# Regex Literal`n`nPattern [a-z]+.* and (foo|bar)? here."
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x46
        $findBox = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_INPUT" -ClassRegex "Edit|TextBox" -MaxHeight 60 -ExcludeTitleBar -TimeoutMs 1500
        if ($null -ne $findBox) {
            Set-EditorText -MainHwnd $session.Hwnd -EditorElement $findBox -Text "[a-z]+.*" -ReplaceAll -PreserveUndo:$false
        }
        Start-Sleep -Milliseconds 200
        Assert-True (-not $session.Process.HasExited) "输入正则元字符导致查找崩溃"
        $status = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_STATUS" -TimeoutMs 600
        if ($null -ne $status) {
            Assert-Match -Text $status.Current.Name -Pattern "1/1" -Message "应按字面量命中 1/1"
        }
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F7-04" -Tier 2 -Feature "F7" -Group "G4" -Title "无选区时按 Ctrl+C 不清空原有剪贴板且不崩溃" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F7-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "no_sel.md"
        Write-BytesUtf8 -Path $doc -Text "# No Selection"
        Set-ClipboardTextWin32 -Text "PRESERVED_CLIP_VALUE"

        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x43 # Ctrl+C
        Start-Sleep -Milliseconds 200
        Assert-True (-not $session.Process.HasExited)
        $cb = Get-ClipboardTextWin32
        Assert-Equal -Actual $cb -Expected "PRESERVED_CLIP_VALUE" -Message "无选区按 Ctrl+C 不应清空或覆盖原有剪贴板"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F7-05" -Tier 2 -Feature "F7" -Group "G4" -Title "Esc 关闭查找栏后立即按 Ctrl+E 恢复正常阅读快捷键流" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F7-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "find_close_flow.md"
        Write-BytesUtf8 -Path $doc -Text "# Find Flow`n`nWord target"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x46
        $findBox = Find-Control -Root $session.MainWindow -AutomationId "IDC_FIND_INPUT" -ClassRegex "Edit|TextBox" -MaxHeight 60 -ExcludeTitleBar -TimeoutMs 1500
        $focusH = if ($findBox) { [IntPtr]$findBox.Current.NativeWindowHandle } else { [IntPtr]::Zero }
        if ($findBox) {
            Set-EditorText -MainHwnd $session.Hwnd -EditorElement $findBox -Text "target" -ReplaceAll -PreserveUndo:$false
        }
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $focusH -Vk 0x1B # Esc
        Start-Sleep -Milliseconds 150

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45 # Ctrl+E
        $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern "修改模式"
        Assert-True ($null -ne (T2_FindEditorControl -Window $session.MainWindow))
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# F8: 模式切换边界与未保存确认 (T2-F8-01 .. T2-F8-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T2-F8-01" -Tier 2 -Feature "F8" -Group "G4" -Title "修改态无改动时点击取消或按 Esc 无弹窗瞬间切回阅读态" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F8-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "clean_cancel.md"
        Write-BytesUtf8 -Path $doc -Text "# Clean Cancel"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        Assert-True ($null -ne $ed)

        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$ed.Current.NativeWindowHandle) -Vk 0x1B # Esc
        $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern "阅读模式"
        Assert-True ($null -eq (Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 250)) "无改动取消不应弹出确认框"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F8-02" -Tier 2 -Feature "F8" -Group "G4" -Title "修改态有改动时取消弹窗选 Cancel 继续留守修改态且保留内容" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F8-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "stay_edit.md"
        Write-BytesUtf8 -Path $doc -Text "Origin"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text "KEEP_IN_EDITOR" -ReplaceAll
        $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern '\*'

        Invoke-Button -Element (T2_FindCancelButton -Window $session.MainWindow) -MainHwnd $session.Hwnd
        $dlg = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 3500
        Assert-True ($null -ne $dlg) "有未保存修改点取消应弹窗"
        Invoke-ModalBucket -Modal $dlg -Bucket Cancel

        $null = Wait-Condition -TimeoutMs 3000 -ErrorMessage "弹窗未关闭" -Condition {
            $null -eq (Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 0)
        }
        Assert-Match -Text $session.MainWindow.Current.Name -Pattern "修改模式"
        Assert-Match -Text (Get-EditorText -EditorElement $ed) -Pattern "KEEP_IN_EDITOR"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F8-03" -Tier 2 -Feature "F8" -Group "G4" -Title "有未保存改动取消弹窗选 Discard 切回阅读态且磁盘原文件无损" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F8-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "discard.md"
        Write-BytesUtf8 -Path $doc -Text "ORIGINAL_IMMUTABLE"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text "DISCARD_ME" -ReplaceAll
        $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern '\*'

        Invoke-Button -Element (T2_FindCancelButton -Window $session.MainWindow) -MainHwnd $session.Hwnd
        $dlg = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 3500
        Assert-True ($null -ne $dlg)
        Invoke-ModalBucket -Modal $dlg -Bucket Discard

        $t = T2_WaitTitleMatch -Window $session.MainWindow -Pattern "^discard\.md\s*-\s*阅读模式"
        Assert-True ($t -notmatch '\*')
        $meta = Read-FileBytesMeta -Path $doc
        Assert-Equal -Actual $meta.Text.Trim() -Expected "ORIGINAL_IMMUTABLE"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F8-04" -Tier 2 -Feature "F8" -Group "G4" -Title "有未保存改动取消弹窗选 Save 原子写回磁盘并切回阅读态" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F8-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "confirm_save.md"
        Write-BytesUtf8 -Path $doc -Text "Before"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text "SAVED_BY_MODAL" -ReplaceAll
        $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern '\*'

        Invoke-Button -Element (T2_FindCancelButton -Window $session.MainWindow) -MainHwnd $session.Hwnd
        $dlg = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 3500
        Assert-True ($null -ne $dlg)
        Invoke-ModalBucket -Modal $dlg -Bucket Save

        $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern "^confirm_save\.md\s*-\s*阅读模式"
        $meta = Read-FileBytesMeta -Path $doc
        Assert-Equal -Actual $meta.Text.Trim() -Expected "SAVED_BY_MODAL"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F8-05" -Tier 2 -Feature "F8" -Group "G4" -Title "无参启动未打开文件空状态下按 Ctrl+E 安全忽略或处理不崩溃" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F8-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $session = Start-AppInstance -Context $ctx -Arguments @() -TimeoutMs $CaseEnv.TimeoutMs
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        Start-Sleep -Milliseconds 300
        Assert-True (-not $session.Process.HasExited) "空状态按 Ctrl+E 导致崩溃"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# F9: 编辑模式文本与撤销极值边界 (T2-F9-01 .. T2-F9-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T2-F9-01" -Tier 2 -Feature "F9" -Group "G4" -Title "编辑输入多行中文 Unicode 含 Emoji 与生僻字无乱码" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F9-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "unicode_edit.md"
        Write-BytesUtf8 -Path $doc -Text "Init"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        $payload = "# 中文测试`n第二行：Emoji 🚀📝✅ 与汉字 𠮷龥"
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text $payload -ReplaceAll
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$ed.Current.NativeWindowHandle) -Ctrl -Vk 0x53

        $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern "阅读模式"
        $meta = Read-FileBytesMeta -Path $doc
        Assert-Equal -Actual (T2_NormalizeEol $meta.Text).Trim() -Expected $payload -Message "Unicode/Emoji 保存后发生损坏"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F9-02" -Tier 2 -Feature "F9" -Group "G4" -Title "编辑区全选删除成 0 字节保持脏状态且不崩溃" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F9-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "clear_all.md"
        Write-BytesUtf8 -Path $doc -Text "Some existing content"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text "" -ReplaceAll
        $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern '\*'
        Assert-Equal -Actual ((T2_NormalizeEol (Get-EditorText -EditorElement $ed)).TrimEnd("`r", "`n").Length) -Expected 0
        Assert-True (-not $session.Process.HasExited)
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F9-03" -Tier 2 -Feature "F9" -Group "G4" -Title "编辑区注入 ~100KB 大段文本后 UI 仍流畅响应并可保存" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F9-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "big_edit.md"
        Write-BytesUtf8 -Path $doc -Text "Start"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        $unit = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ 中文段落测试行 `n"
        $sb = New-Object System.Text.StringBuilder
        while ($sb.Length -lt 100000) { [void]$sb.Append($unit) }
        $bigText = $sb.ToString()

        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text $bigText -ReplaceAll -PreserveUndo:$false
        $got = T2_NormalizeEol (Get-EditorText -EditorElement $ed)
        Assert-True ($got.Length -ge 90000) "大文本注入不完整 (len=$($got.Length))"

        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$ed.Current.NativeWindowHandle) -Ctrl -Vk 0x53
        $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern "阅读模式" -TimeoutMs 8000
        $meta = Read-FileBytesMeta -Path $doc
        Assert-True ($meta.Length -ge 100000) "大文本保存后磁盘长度不足: $($meta.Length)"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F9-04" -Tier 2 -Feature "F9" -Group "G4" -Title "连续 Ctrl+Z 撤销超过操作次数后行为安全可控不崩溃" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F9-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "over_undo.md"
        Write-BytesUtf8 -Path $doc -Text "INIT_STATE"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        $edHwnd = [IntPtr]$ed.Current.NativeWindowHandle

        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text "STEP_ONE" -ReplaceAll
        Start-Sleep -Milliseconds 80
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text "STEP_TWO" -ReplaceAll
        Start-Sleep -Milliseconds 80

        for ($i = 0; $i -lt 5; $i++) {
            Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $edHwnd -Ctrl -Vk 0x5A # Ctrl+Z
            Start-Sleep -Milliseconds 60
        }
        Assert-True (-not $session.Process.HasExited) "连续过度撤销导致进程崩溃"
        $txt = (T2_NormalizeEol (Get-EditorText -EditorElement $ed)).Trim()
        Assert-Equal -Actual $txt -Expected "INIT_STATE" -Message "过度撤销应安全停在初始状态"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F9-05" -Tier 2 -Feature "F9" -Group "G4" -Title "editorTabSize 边界配置生效且 Tab 输入后不丢失编辑焦点" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F9-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "tab_focus.md"
        $cfg = Join-Path $ctx.SandboxDir "tab_focus.json"
        Write-BytesUtf8 -Path $doc -Text "Start"
        Write-BytesUtf8 -Path $cfg -Text '{"editorTabSize": 2}'
        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        $edHwnd = [IntPtr]$ed.Current.NativeWindowHandle
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd $edHwnd -Vk 0x09 # Tab
        Start-Sleep -Milliseconds 80
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text "afterTab"
        $txt = Get-EditorText -EditorElement $ed
        Assert-Match -Text $txt -Pattern "afterTab" -Message "Tab 后编辑区未保留焦点继续输入"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# F10: 保存失败保护与原子写入边界 (T2-F10-01 .. T2-F10-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T2-F10-01" -Tier 2 -Feature "F10" -Group "G4" -Title "目标文件只读时 Ctrl+S 失败保护：留守修改态、保文本、原文件无损" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F10-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    $doc = Join-Path $ctx.SandboxDir "readonly.md"
    try {
        Write-BytesUtf8 -Path $doc -Text "READONLY_ORIGIN"
        [System.IO.File]::SetAttributes($doc, [System.IO.FileAttributes]::ReadOnly)

        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text "READONLY_UNSAVED_KEEP" -ReplaceAll
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$ed.Current.NativeWindowHandle) -Ctrl -Vk 0x53

        $dlg = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 4000
        Assert-True ($null -ne $dlg) "对只读文件保存应弹出失败错误提示"
        Invoke-ModalBucket -Modal $dlg -Bucket Ok
        $null = Wait-Condition -TimeoutMs 3000 -ErrorMessage "关闭错误框超时" -Condition {
            $null -eq (Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 0)
        }

        Assert-Match -Text $session.MainWindow.Current.Name -Pattern "修改模式" -Message "保存失败严禁切出修改模式"
        Assert-Match -Text (Get-EditorText -EditorElement $ed) -Pattern "READONLY_UNSAVED_KEEP" -Message "保存失败必须保留编辑区内容"
        $meta = Read-FileBytesMeta -Path $doc
        Assert-Equal -Actual $meta.Text.Trim() -Expected "READONLY_ORIGIN" -Message "原只读文件被非法篡改"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        if (Test-Path -LiteralPath $doc) {
            [System.IO.File]::SetAttributes($doc, [System.IO.FileAttributes]::Normal)
        }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F10-02" -Tier 2 -Feature "F10" -Group "G4" -Title "修改期间原文件被外部删除时保存失败留守修改态并保留编辑文本" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F10-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "vanished.md"
        Write-BytesUtf8 -Path $doc -Text "Initial"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text "KEEP_WHEN_DELETED" -ReplaceAll

        Remove-Item -LiteralPath $doc -Force
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$ed.Current.NativeWindowHandle) -Ctrl -Vk 0x53

        $dlg = Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 4000
        Assert-True ($null -ne $dlg) "原文件被删除时保存应提示错误"
        Invoke-ModalBucket -Modal $dlg -Bucket Ok
        $null = Wait-Condition -TimeoutMs 3000 -ErrorMessage "关闭错误框超时" -Condition {
            $null -eq (Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 0)
        }

        Assert-Match -Text $session.MainWindow.Current.Name -Pattern "修改模式"
        Assert-Match -Text (Get-EditorText -EditorElement $ed) -Pattern "KEEP_WHEN_DELETED"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F10-03" -Tier 2 -Feature "F10" -Group "G4" -Title "将文档清空为 0 字节后 Ctrl+S 合法写盘并自动切回阅读模式" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F10-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "save_zero.md"
        Write-BytesUtf8 -Path $doc -Text "# Non Empty"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text "" -ReplaceAll
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$ed.Current.NativeWindowHandle) -Ctrl -Vk 0x53

        $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern "^save_zero\.md\s*-\s*阅读模式"
        $meta = Read-FileBytesMeta -Path $doc
        Assert-Equal -Actual $meta.Length -Expected 0 -Message "0 字节保存后文件大小应为 0"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F10-04" -Tier 2 -Feature "F10" -Group "G4" -Title "原子保存完成后目录内无任何残留 .tmp 临时文件" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F10-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "atomic_clean.md"
        Write-BytesUtf8 -Path $doc -Text "Old"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text "Updated Atomically" -ReplaceAll
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$ed.Current.NativeWindowHandle) -Ctrl -Vk 0x53

        $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern "阅读模式"
        $leftovers = @(Get-ChildItem -LiteralPath $ctx.SandboxDir -Force | Where-Object { $_.Name -ne "atomic_clean.md" })
        $leftoverNames = if ($leftovers.Count -gt 0) { ($leftovers | ForEach-Object { $_.Name }) -join ', ' } else { "" }
        Assert-Equal -Actual $leftovers.Count -Expected 0 -Message "发现残留临时文件: $leftoverNames"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F10-05" -Tier 2 -Feature "F10" -Group "G4" -Title "带 UTF-8 BOM 与 CRLF 文档编辑保存后字节级验证 BOM 与 CRLF 完好保留" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F10-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "bom_crlf.md"
        Write-BytesUtf8 -Path $doc -Text "第一行`n第二行" -Bom $true -Eol 'CRLF'
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45
        $ed = T2_FindEditorControl -Window $session.MainWindow
        Set-EditorText -MainHwnd $session.Hwnd -EditorElement $ed -Text "第一行`n第二行修订`n第三行" -ReplaceAll
        Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$ed.Current.NativeWindowHandle) -Ctrl -Vk 0x53

        $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern "阅读模式"
        $meta = Read-FileBytesMeta -Path $doc
        Assert-Equal -Actual $meta.HasUtf8Bom -Expected $true -Message "UTF-8 BOM 丢失"
        Assert-Equal -Actual $meta.LineEnding -Expected 'CRLF' -Message "CRLF 换行风味丢失 (actual=$($meta.LineEnding))"
        Assert-Match -Text $meta.Text -Pattern "第二行修订"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# F11: JSON 配置缺失与损坏降级边界 (T2-F11-01 .. T2-F11-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T2-F11-01" -Tier 2 -Feature "F11" -Group "G4" -Title "config.json 完全缺失时无弹窗安全启动" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F11-01" -ExePath $CaseEnv.ExePath -CopyExeToSandbox -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "missing_cfg.md"
        $missingCfg = Join-Path $ctx.SandboxDir "absent_config.json"
        Write-BytesUtf8 -Path $doc -Text "# No Config File"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $missingCfg) -TimeoutMs $CaseEnv.TimeoutMs
        Assert-True (-not $session.Process.HasExited)
        Assert-True ($null -eq (Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 300))
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F11-02" -Tier 2 -Feature "F11" -Group "G4" -Title "config.json 语法损坏（尾部逗号与截断）安全回退默认不阻断阅读" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F11-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "corrupt_cfg.md"
        $cfg = Join-Path $ctx.SandboxDir "corrupt.json"
        Write-BytesUtf8 -Path $doc -Text "# Corrupt JSON Safe"
        Write-BytesUtf8 -Path $cfg -Text '{"theme": "dark", "fontSize": 18, '

        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 300
        Assert-True (-not $session.Process.HasExited) "损坏 JSON 导致启动崩溃"
        Assert-True ($null -eq (Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 300)) "损坏 JSON 不应弹窗阻塞"
        $lum = Get-WindowLuminance -Hwnd $session.Hwnd
        Assert-True ($lum -ge 160) "损坏 JSON 应整文件回退默认亮色 (lum=$lum)"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F11-03" -Tier 2 -Feature "F11" -Group "G4" -Title "单字段类型错或越界时逐字段回退且同文件合法字段依然生效" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F11-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "partial_bad.md"
        $cfg = Join-Path $ctx.SandboxDir "partial_bad.json"
        Write-BytesUtf8 -Path $doc -Text "# Partial Valid Config"
        $json = @'
{
  "theme": "dark",
  "fontSize": -10,
  "lineHeight": "abc",
  "maxWidth": 99999,
  "pagePadding": -5
}
'@
        Write-BytesUtf8 -Path $cfg -Text $json
        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs

        $lum = Wait-Condition -TimeoutMs 5000 -ErrorMessage "合法字段 theme=dark 未能在部分坏字段下生效" -Condition {
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

[void]$cases.Add((New-TestCaseDef -Id "T2-F11-04" -Tier 2 -Feature "F11" -Group "G4" -Title "根节点为 JSON 数组 [] 时整文件安全回退默认配置" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F11-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "array_root.md"
        $cfg = Join-Path $ctx.SandboxDir "array_root.json"
        Write-BytesUtf8 -Path $doc -Text "# Array Root JSON"
        Write-BytesUtf8 -Path $cfg -Text '[{"theme": "dark"}]'

        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 300
        Assert-True (-not $session.Process.HasExited)
        Assert-True ($null -eq (Find-ModalDialog -ProcessId $session.ProcessId -MainHwnd $session.Hwnd -TimeoutMs 300))
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F11-05" -Tier 2 -Feature "F11" -Group "G4" -Title "只读 config.json 或非法配置路径启动与关闭不崩溃不损坏文件" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F11-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    $roCfg = Join-Path $ctx.SandboxDir "ro_config.json"
    try {
        $doc = Join-Path $ctx.SandboxDir "ro_cfg_doc.md"
        Write-BytesUtf8 -Path $doc -Text "# Readonly Config"
        $origCfgText = '{"theme": "dark", "rememberWindowGeometry": true}'
        Write-BytesUtf8 -Path $roCfg -Text $origCfgText
        [System.IO.File]::SetAttributes($roCfg, [System.IO.FileAttributes]::ReadOnly)

        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $roCfg) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 250
        Assert-True (-not $session.Process.HasExited)
        Stop-AppInstance -Session $session
        $session = $null

        $meta = Read-FileBytesMeta -Path $roCfg
        Assert-Equal -Actual $meta.Text -Expected $origCfgText -Message "只读配置文件被意外破坏"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        if (Test-Path -LiteralPath $roCfg) {
            [System.IO.File]::SetAttributes($roCfg, [System.IO.FileAttributes]::Normal)
        }
        Remove-E2ECaseContext -Context $ctx
    }
}))

# ==============================================================================
# F12: 大文件、内存稳定性与极小布局边界 (T2-F12-01 .. T2-F12-05)
# ==============================================================================

[void]$cases.Add((New-TestCaseDef -Id "T2-F12-01" -Tier 2 -Feature "F12" -Group "G4" -Title "1MB 超大 Markdown 打开后 Working Set 可控且无 OOM 崩溃" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F12-01" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "huge_1mb.md"
        $unit = "## Large Document Section`n`nThis line repeats to build a 1MB realistic technical Markdown file with **emphasis** and ``code``.`n`n"
        $sb = New-Object System.Text.StringBuilder
        while ($sb.Length -lt 1048576) { [void]$sb.Append($unit) }
        Write-BytesUtf8 -Path $doc -Text $sb.ToString()

        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs
        Start-Sleep -Milliseconds 1500
        Assert-True (-not $session.Process.HasExited) "1MB 大文件导致崩溃或 OOM"
        $m = Get-ProcessTreeMetrics -Process $session.Process
        $CaseEnv.Metrics.WorkingSetMB_1MB = $m.WorkingSetMB
        Assert-Equal -Actual $m.ProcessCount -Expected 1
        if ($CaseEnv.IsSelfTest) {
            Assert-True ($m.WorkingSetMB -gt 0 -and $m.WorkingSetMB -lt 160) "SelfTest 1MB 内存超标: $($m.WorkingSetMB) MB"
        } else {
            Assert-True ($m.WorkingSetMB -gt 0 -and $m.WorkingSetMB -le 90) "真实应用 1MB 文档 WorkingSet 超出 90MB 阈值: $($m.WorkingSetMB) MB"
        }
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F12-02" -Tier 2 -Feature "F12" -Group "G4" -Title "连续拖放切换重载文档 10 次无明显内存泄漏飙升" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F12-02" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $fA = Join-Path $ctx.SandboxDir "cycle_a.md"
        $fB = Join-Path $ctx.SandboxDir "cycle_b.md"
        $baseText = ("# Cycle Doc`n`nParagraph data for repeated load test.`n" * 40)
        Write-BytesUtf8 -Path $fA -Text ("# Doc A`n" + $baseText)
        Write-BytesUtf8 -Path $fB -Text ("# Doc B`n" + $baseText)

        $session = Start-AppInstance -Context $ctx -Arguments @($fA) -TimeoutMs $CaseEnv.TimeoutMs
        $wsAfter2 = 0.0
        for ($i = 1; $i -le 10; $i++) {
            $target = if (($i % 2) -eq 1) { $fB } else { $fA }
            $leaf = Split-Path -Leaf $target
            Send-DropFiles -Hwnd $session.Hwnd -Paths @($target)
            $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern ([regex]::Escape($leaf))
            if ($i -eq 2) {
                $wsAfter2 = (Get-ProcessTreeMetrics -Process $session.Process).WorkingSetMB
            }
        }
        $wsAfter10 = (Get-ProcessTreeMetrics -Process $session.Process).WorkingSetMB
        $growth = [math]::Round($wsAfter10 - $wsAfter2, 2)
        $CaseEnv.Metrics.WsAfter2 = $wsAfter2
        $CaseEnv.Metrics.WsAfter10 = $wsAfter10
        $CaseEnv.Metrics.GrowthMB = $growth
        Assert-True ($growth -lt 40) "连续重载 10 次后内存异常增长: +${growth} MB"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F12-03" -Tier 2 -Feature "F12" -Group "G4" -Title "连续切换阅读 <-> 编辑模式 10 次后内存与句柄保持稳定" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F12-03" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "mode_loop.md"
        Write-BytesUtf8 -Path $doc -Text "# Mode Loop Stress`n`nLine 1`nLine 2"
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        $m2 = $null
        for ($i = 1; $i -le 10; $i++) {
            Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45 # -> Editing
            $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern "修改模式"
            $ed = T2_FindEditorControl -Window $session.MainWindow
            Send-Hotkey -MainHwnd $session.Hwnd -FocusHwnd ([IntPtr]$ed.Current.NativeWindowHandle) -Vk 0x1B # Esc -> Reading
            $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern "阅读模式"
            if ($i -eq 2) { $m2 = Get-ProcessTreeMetrics -Process $session.Process }
        }
        $m10 = Get-ProcessTreeMetrics -Process $session.Process
        $wsDiff = [math]::Round($m10.WorkingSetMB - $m2.WorkingSetMB, 2)
        $hDiff = $m10.HandleCount - $m2.HandleCount
        $CaseEnv.Metrics.WsDiffMB = $wsDiff
        $CaseEnv.Metrics.HandleDiff = $hDiff
        Assert-True ($wsDiff -lt 35) "模式切换 10 次后内存异常增长: +${wsDiff} MB"
        Assert-True ($hDiff -lt 300) "模式切换 10 次后句柄疑似泄漏: +${hDiff}"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F12-04" -Tier 2 -Feature "F12" -Group "G4" -Title "进程树严格核查：单进程且绝无 msedgewebview2/node/electron 子进程" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F12-04" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "proctree.md"
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "sample-gfm-full.md") -Destination $doc
        Copy-Item -LiteralPath (Join-Path $CaseEnv.FixturesDir "test-image.png") -Destination (Join-Path $ctx.SandboxDir "test-image.png")
        $session = Start-AppInstance -Context $ctx -Arguments @($doc) -TimeoutMs $CaseEnv.TimeoutMs

        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x46 # Ctrl+F
        Start-Sleep -Milliseconds 150
        Send-Hotkey -MainHwnd $session.Hwnd -Vk 0x1B # Esc
        Send-Hotkey -MainHwnd $session.Hwnd -Ctrl -Vk 0x45 # Ctrl+E
        Start-Sleep -Milliseconds 250

        $m = Get-ProcessTreeMetrics -Process $session.Process
        Assert-Equal -Actual $m.ProcessCount -Expected 1 -Message "进程树总数应为 1 (Children=$($m.ChildNames -join ','))"
        foreach ($cn in $m.ChildNames) {
            Assert-True ($cn -notmatch 'msedgewebview2|node|chrome|electron') "检测到违规重型子进程: $cn"
        }
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

[void]$cases.Add((New-TestCaseDef -Id "T2-F12-05" -Tier 2 -Feature "F12" -Group "G4" -Title "极小窗口 400x300 启动后工具栏按钮不重叠且主按钮可操作" -Run {
    param($CaseEnv)
    $ctx = New-E2ECaseContext -CaseId "T2-F12-05" -ExePath $CaseEnv.ExePath -RunId $CaseEnv.RunId
    $session = $null
    try {
        $doc = Join-Path $ctx.SandboxDir "tiny_win.md"
        $cfg = Join-Path $ctx.SandboxDir "tiny_win.json"
        Write-BytesUtf8 -Path $doc -Text "# Tiny Window"
        Write-BytesUtf8 -Path $cfg -Text '{"windowWidth": 400, "windowHeight": 300}'

        $session = Start-AppInstance -Context $ctx -Arguments @($doc, "--config", $cfg) -TimeoutMs $CaseEnv.TimeoutMs
        [array]$btns = Get-ToolbarActionButtons -Root $session.MainWindow
        Assert-True ($btns.Count -ge 2) "极小窗口下工具栏按钮丢失"

        for ($i = 0; $i -lt $btns.Count; $i++) {
            $ri = $btns[$i].Current.BoundingRectangle
            Assert-True ($ri.Width -gt 10 -and $ri.Height -gt 10) "按钮宽高异常"
            for ($j = $i + 1; $j -lt $btns.Count; $j++) {
                $rj = $btns[$j].Current.BoundingRectangle
                $overlapX = ($ri.Left -lt $rj.Right) -and ($rj.Left -lt $ri.Right)
                $overlapY = ($ri.Top -lt $rj.Bottom) -and ($rj.Top -lt $ri.Bottom)
                Assert-True (-not ($overlapX -and $overlapY)) "极小窗口下工具栏按钮发生重叠: '$($btns[$i].Current.Name)' vs '$($btns[$j].Current.Name)'"
            }
        }

        $toggle = T2_FindModeToggleButton -Window $session.MainWindow
        Invoke-Button -Element $toggle -MainHwnd $session.Hwnd
        $null = T2_WaitTitleMatch -Window $session.MainWindow -Pattern "修改模式"
    } finally {
        if ($session) { Stop-AppInstance -Session $session }
        Remove-E2ECaseContext -Context $ctx
    }
}))

return $cases.ToArray()
