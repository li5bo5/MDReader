#Requires -Version 5.1
<#
.SYNOPSIS
    MDReader E2E 自动化测试统一运行器 (run-e2e.ps1)
.DESCRIPTION
    支持针对真实 mdreader.exe (-ExePath) 或测试框架自检替身 (-SelfTest) 执行 Tier 1..4 测试套件，
    负责崩溃残留注册表恢复、沙盒清理、进程树防游离及输出 e2e-report.json / e2e-report.md。
#>

[CmdletBinding()]
param(
    [string]$ExePath = "",
    [ValidateSet("1", "2", "3", "4", "All")]
    [string]$Tier = "All",
    [string]$Filter = "*",
    [string]$OutDir = "",
    [int]$TimeoutSec = 15,
    [switch]$SelfTest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptDir = $PSScriptRoot
$repoRoot = (Resolve-Path (Join-Path $scriptDir "..\..")).Path
$harnessPath = Join-Path $scriptDir "lib\E2EHarness.psm1"
$mockAppScriptPath = Join-Path $scriptDir "lib\MockApp.ps1"
$fixturesDir = Join-Path $scriptDir "fixtures"

if (-not $OutDir) {
    $OutDir = Join-Path $scriptDir "reports"
}

if (-not (Test-Path -LiteralPath $harnessPath)) {
    Write-Error "缺少核心驱动库: $harnessPath"
    exit 1
}

Import-Module $harnessPath -Force -DisableNameChecking

# 1. 启动前优先核查并恢复上次异常中断遗留的 HKCU 注册表状态
try {
    $recovered = Restore-DanglingRegistryIfAny
    if ($recovered) {
        Write-Host "[恢复] 检测到残留注册表崩溃快照，已自动完成 HKCU 回滚。" -ForegroundColor Yellow
    }
} catch {
    Write-Error "恢复残留注册表快照失败: $($_.Exception.Message)"
    exit 1
}

$runId = [Guid]::NewGuid().ToString("N").Substring(0, 8)
$selfTestTempDir = ""
$effectiveExe = ""
$overallSw = [System.Diagnostics.Stopwatch]::StartNew()
$caseResults = New-Object 'System.Collections.Generic.List[object]'
$exitCode = 0

try {
    # 2. 确定被测可执行文件路径（-SelfTest 预编译 MockApp 或校验真实 -ExePath）
    if ($SelfTest) {
        if (-not (Test-Path -LiteralPath $mockAppScriptPath)) {
            throw "缺少自检替身脚本: $mockAppScriptPath"
        }
        $selfTestTempDir = Join-Path $env:TEMP ("mdreader_selftest_" + $runId)
        New-Item -ItemType Directory -Force -Path $selfTestTempDir | Out-Null
        $mockExeTarget = Join-Path $selfTestTempDir "MockMdReader.exe"
        Write-Host "[SelfTest] 正在编译轻量框架自检替身 -> $mockExeTarget ..." -ForegroundColor Cyan
        . $mockAppScriptPath
        $effectiveExe = Compile-MockAppExe -OutExePath $mockExeTarget
        Write-Host "[SelfTest] 编译完成: $effectiveExe" -ForegroundColor Cyan
    } else {
        if (-not $ExePath) {
            throw "非 -SelfTest 模式下必须提供 -ExePath 参数指向被测 mdreader.exe。"
        }
        if (-not (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
            throw "指定的被测可执行文件不存在: $ExePath"
        }
        $effectiveExe = (Resolve-Path -LiteralPath $ExePath).Path
    }

    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    $screenshotDir = Join-Path $OutDir "screenshots"

    # 3. 发现并加载 Tier*.Tests.ps1
    $targetTiers = if ($Tier -eq "All") { @(1, 2, 3, 4) } else { @([int]$Tier) }
    $loadedCases = New-Object 'System.Collections.Generic.List[object]'
    $foundFilesCount = 0

    foreach ($t in $targetTiers) {
        $tierFile = Join-Path $scriptDir ("tests\Tier{0}.Tests.ps1" -f $t)
        if (Test-Path -LiteralPath $tierFile -PathType Leaf) {
            $foundFilesCount++
            Write-Host "[加载] $tierFile" -ForegroundColor Gray
            $items = & $tierFile
            foreach ($item in @($items)) {
                if ($null -ne $item -and $item.PSObject.Properties['Id'] -and $item.PSObject.Properties['Run']) {
                    [void]$loadedCases.Add($item)
                }
            }
        }
    }

    if ($foundFilesCount -eq 0) {
        Write-Host "[提示] 未在 tests\e2e\tests\ 下发现 Tier*.Tests.ps1 用例文件，跳过用例执行并生成报告框架。" -ForegroundColor Yellow
    }

    # 4. 按 -Filter 过滤用例
    $selectedCases = @()
    if ($loadedCases.Count -gt 0) {
        $selectedCases = @($loadedCases | Where-Object { $_.Id -like $Filter -or $_.Title -like $Filter })
        if ($selectedCases.Count -eq 0) {
            Write-Host "[警告] 使用 -Filter '$Filter' 未匹配到任何测试用例（已加载总计 $($loadedCases.Count) 条）。" -ForegroundColor Yellow
        }
    }

    Write-Host ("[执行] 模式={0}, Tier={1}, 匹配用例数={2}, 默认超时={3}s" -f ($(if ($SelfTest) { "SelfTest" } else { "RealExe" }), $Tier, $selectedCases.Count, $TimeoutSec)) -ForegroundColor White

    # 5. 串行执行各用例并确保绝对隔离清理
    foreach ($c in $selectedCases) {
        $caseSw = [System.Diagnostics.Stopwatch]::StartNew()
        $status = "PASS"
        $errMsg = ""
        $stack = ""
        $shotPath = ""
        $caseEnv = @{
            ExePath     = $effectiveExe
            RepoRoot    = $repoRoot
            FixturesDir = $fixturesDir
            TimeoutSec  = $TimeoutSec
            TimeoutMs   = ($TimeoutSec * 1000)
            OutDir      = $OutDir
            IsSelfTest  = [bool]$SelfTest
            RunId       = $runId
            Metrics     = @{}
        }

        Write-Host ("  -> [{0}] {1} ({2}) ... " -f $c.Id, $c.Title, $c.Feature) -NoNewline
        try {
            $ret = & $c.Run $caseEnv
            if ($ret -is [hashtable] -and $ret.ContainsKey('Metrics')) {
                $caseEnv.Metrics = $ret.Metrics
            }
            $caseSw.Stop()
            Write-Host ("PASS ({0} ms)" -f $caseSw.ElapsedMilliseconds) -ForegroundColor Green
        } catch {
            $caseSw.Stop()
            $status = "FAIL"
            $errMsg = $_.Exception.Message
            $stack = $_.ScriptStackTrace
            $exitCode = 1
            Write-Host ("FAIL ({0} ms) - {1}" -f $caseSw.ElapsedMilliseconds, $errMsg) -ForegroundColor Red

            # 尝试在清理前捕获残留窗口截图辅助排查
            try {
                $exeLeaf = [System.IO.Path]::GetFileNameWithoutExtension($effectiveExe)
                $liveProcs = @(Get-Process -Name $exeLeaf -ErrorAction SilentlyContinue)
                foreach ($lp in $liveProcs) {
                    if ($lp.MainWindowHandle -ne [IntPtr]::Zero) {
                        New-Item -ItemType Directory -Force -Path $screenshotDir | Out-Null
                        $candidatePng = Join-Path $screenshotDir ("{0}.png" -f ($c.Id -replace '[^a-zA-Z0-9_\-]', '_'))
                        $shotPath = Capture-WindowPng -Hwnd $lp.MainWindowHandle -OutPath $candidatePng
                        break
                    }
                }
            } catch {}
        } finally {
            try { $null = Restore-DanglingRegistryIfAny } catch {}
            try { Invoke-AppCleanupAll } catch {}
        }

        [void]$caseResults.Add([PSCustomObject]@{
            Id             = $c.Id
            Tier           = $c.Tier
            Feature        = $c.Feature
            Title          = $c.Title
            Group          = $c.Group
            Status         = $status
            DurationMs     = $caseSw.ElapsedMilliseconds
            Error          = $errMsg
            StackTrace     = $stack
            ScreenshotPath = $shotPath
            Metrics        = $caseEnv.Metrics
        })
    }
} catch {
    $exitCode = 1
    Write-Host ("[错误] 运行器异常: {0}" -f $_.Exception.Message) -ForegroundColor Red
} finally {
    try { $null = Restore-DanglingRegistryIfAny } catch {}
    try { Invoke-AppCleanupAll } catch {}
    if ($selfTestTempDir -and (Test-Path -LiteralPath $selfTestTempDir)) {
        try { Remove-Item -LiteralPath $selfTestTempDir -Recurse -Force -ErrorAction SilentlyContinue } catch {}
    }
}

$overallSw.Stop()

# 6. 生成 e2e-report.json 与 e2e-report.md
$totalCount = $caseResults.Count
$passCount = 0
$failCount = 0
foreach ($cr in $caseResults) {
    if ($cr.Status -eq "PASS") { $passCount++ }
    elseif ($cr.Status -eq "FAIL") { $failCount++ }
}

$reportObj = [PSCustomObject]@{
    GeneratedAtUtc = (Get-Date).ToUniversalTime().ToString("o")
    RunId          = $runId
    Mode           = if ($SelfTest) { "SelfTest" } else { "RealExe" }
    ExePath        = $effectiveExe
    Tier           = $Tier
    Filter         = $Filter
    TimeoutSec     = $TimeoutSec
    Environment    = [PSCustomObject]@{
        OS         = [System.Environment]::OSVersion.VersionString
        PowerShell = $PSVersionTable.PSVersion.ToString()
        Is64Bit    = [System.Environment]::Is64BitProcess
    }
    Summary        = [PSCustomObject]@{
        Total       = $totalCount
        Passed      = $passCount
        Failed      = $failCount
        DurationSec = [math]::Round($overallSw.Elapsed.TotalSeconds, 2)
        ExitCode    = $exitCode
    }
    Cases          = $caseResults.ToArray()
}

if (Test-Path -LiteralPath $OutDir) {
    $jsonOut = Join-Path $OutDir "e2e-report.json"
    $mdOut = Join-Path $OutDir "e2e-report.md"
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllBytes($jsonOut, $utf8NoBom.GetBytes(($reportObj | ConvertTo-Json -Depth 8)))

    $md = New-Object System.Text.StringBuilder
    [void]$md.AppendLine("# MDReader E2E 自动化测试报告")
    [void]$md.AppendLine("")
    [void]$md.AppendLine("- **生成时间 (UTC)**: $($reportObj.GeneratedAtUtc)")
    [void]$md.AppendLine("- **运行模式**: $($reportObj.Mode)")
    [void]$md.AppendLine("- **被测程序**: ``$($reportObj.ExePath)``")
    [void]$md.AppendLine("- **Tier / Filter**: ``$Tier`` / ``$Filter``")
    [void]$md.AppendLine("- **运行环境**: $($reportObj.Environment.OS) | PowerShell $($reportObj.Environment.PowerShell)")
    [void]$md.AppendLine("- **总耗时**: $($reportObj.Summary.DurationSec)s")
    [void]$md.AppendLine("- **执行结果**: 总计 **$totalCount** | 通过 **$passCount** | 失败 **$failCount** | 退出码 **$exitCode**")
    [void]$md.AppendLine("")

    if ($totalCount -gt 0) {
        [void]$md.AppendLine("## 用例明细")
        [void]$md.AppendLine("")
        [void]$md.AppendLine("| ID | Tier | Feature | 标题 | 状态 | 耗时 (ms) | 备注 |")
        [void]$md.AppendLine("|---|---|---|---|---|---:|---|")
        foreach ($r in $caseResults) {
            $note = if ($r.Status -eq "FAIL") { ($r.Error -replace '[\r\n|]+', ' ') } elseif ($r.Metrics -and $r.Metrics.Count -gt 0) { ($r.Metrics | ConvertTo-Json -Compress) } else { "-" }
            [void]$md.AppendLine("| $($r.Id) | T$($r.Tier) | $($r.Feature) | $($r.Title) | $($r.Status) | $($r.DurationMs) | $note |")
        }
        [void]$md.AppendLine("")

        $failedCases = @($caseResults | Where-Object { $_.Status -eq "FAIL" })
        if ($failedCases.Count -gt 0) {
            [void]$md.AppendLine("## 失败诊断详情")
            [void]$md.AppendLine("")
            foreach ($fc in $failedCases) {
                [void]$md.AppendLine("### $($fc.Id) — $($fc.Title)")
                [void]$md.AppendLine("- **错误**: $($fc.Error)")
                if ($fc.ScreenshotPath) { [void]$md.AppendLine("- **现场截图**: ``$($fc.ScreenshotPath)``") }
                if ($fc.StackTrace) {
                    [void]$md.AppendLine('```text')
                    [void]$md.AppendLine($fc.StackTrace)
                    [void]$md.AppendLine('```')
                }
                [void]$md.AppendLine("")
            }
        }
    } else {
        [void]$md.AppendLine("> 尚未发现任何可执行的 ``tests\Tier*.Tests.ps1`` 测试用例。")
    }

    [System.IO.File]::WriteAllBytes($mdOut, $utf8NoBom.GetBytes($md.ToString()))
    Write-Host ("[报告] JSON: $jsonOut") -ForegroundColor Cyan
    Write-Host ("[报告] MD:   $mdOut") -ForegroundColor Cyan
}

Write-Host ("[汇总] Total=$totalCount, Passed=$passCount, Failed=$failCount, ExitCode=$exitCode, Elapsed=$($reportObj.Summary.DurationSec)s") -ForegroundColor $(if ($exitCode -eq 0) { "Green" } else { "Red" })
exit $exitCode
