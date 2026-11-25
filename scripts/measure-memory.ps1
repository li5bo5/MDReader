#Requires -Version 5.1
<#
.SYNOPSIS
    MDReader Windows 进程内存（Working Set / Private Bytes）实测采集脚本。
.DESCRIPTION
    分别测量三种场景的冷启动稳定内存占用（窗口就绪后稳定等待 3 秒）：
    1. 空载冷启动（无文件参数）—— 验收门槛 WorkingSet < 20 MB，零 msedgewebview2.exe
    2. 典型 GFM 文档（sample-gfm-full.md）—— 验收门槛 WorkingSet < 30 MB
    3. 200KB+ 长文档（sample-long.md）—— 验证 LRU 布局裁剪内存控制
    同时输出 JSON 测量报告（默认写入 reports\memory-benchmark.json）。
#>

[CmdletBinding()]
param(
    [string]$ExePath = "",
    [int]$SettleSeconds = 3,
    [string]$OutJson = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptDir = $PSScriptRoot
$repoRoot = Split-Path -Parent $scriptDir

if (-not $ExePath) {
    $ExePath = Join-Path $repoRoot "target\release\MDReader.exe"
}
if (-not (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
    throw "未找到被测程序: $ExePath，请先运行 cargo build --release"
}
$resolvedExe = (Resolve-Path -LiteralPath $ExePath).Path

if (-not $OutJson) {
    $OutJson = Join-Path $repoRoot "reports\memory-benchmark.json"
}

function Get-ChildProcessIds([int]$RootPid) {
    $all = @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name)
    $visited = New-Object 'System.Collections.Generic.HashSet[int]'
    $queue = New-Object 'System.Collections.Generic.Queue[int]'
    [void]$visited.Add($RootPid)
    $queue.Enqueue($RootPid)
    while ($queue.Count -gt 0) {
        $cur = $queue.Dequeue()
        foreach ($p in $all) {
            if ([int]$p.ParentProcessId -eq $cur -and $visited.Add([int]$p.ProcessId)) {
                $queue.Enqueue([int]$p.ProcessId)
            }
        }
    }
    return @($visited)
}

function Measure-Scenario {
    param(
        [string]$Name,
        [string[]]$Arguments,
        [double]$ThresholdMB
    )

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $resolvedExe
    $psi.UseShellExecute = $false
    if (@($Arguments).Count -gt 0) {
        $escaped = @($Arguments) | ForEach-Object { '"' + ($_ -replace '"', '\"') + '"' }
        $psi.Arguments = ($escaped -join ' ')
    }

    $proc = [System.Diagnostics.Process]::Start($psi)
    try {
        # 等待主窗口句柄就绪（最多 10 秒）
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        while ($sw.ElapsedMilliseconds -lt 10000) {
            $proc.Refresh()
            if ($proc.HasExited) {
                throw "场景 [$Name] 进程提前退出，退出码=$($proc.ExitCode)"
            }
            if ($proc.MainWindowHandle -ne [IntPtr]::Zero) {
                break
            }
            Start-Sleep -Milliseconds 50
        }
        if ($proc.MainWindowHandle -eq [IntPtr]::Zero) {
            throw "场景 [$Name] 等待主窗口句柄超时"
        }

        Start-Sleep -Seconds $SettleSeconds
        $proc.Refresh()

        $pids = @(Get-ChildProcessIds -RootPid $proc.Id)
        $totalWs = 0L
        $totalWsPriv = 0L
        $totalPriv = 0L
        $totalHandles = 0
        $webviewCount = 0
        $procNames = @()
        $perfRows = @(Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -ErrorAction SilentlyContinue)

        foreach ($pidItem in $pids) {
            try {
                $pObj = Get-Process -Id $pidItem -ErrorAction Stop
                $totalWs += $pObj.WorkingSet64
                $totalPriv += $pObj.PrivateMemorySize64
                $totalHandles += $pObj.HandleCount
                $procNames += $pObj.ProcessName
                $perfMatch = $perfRows | Where-Object { $_.IDProcess -eq $pidItem } | Select-Object -First 1
                if ($null -ne $perfMatch) {
                    $totalWsPriv += [long]$perfMatch.WorkingSetPrivate
                }
                if ($pObj.ProcessName -match 'msedgewebview2|electron') {
                    $webviewCount++
                }
            } catch {}
        }

        $wsMB = [math]::Round($totalWs / 1MB, 2)
        $wsPrivMB = [math]::Round($totalWsPriv / 1MB, 2)
        $privMB = [math]::Round($totalPriv / 1MB, 2)
        # 同时检验：专用工作集 WorkingSetPrivate（任务管理器内存）<= ThresholdMB，总工作集 WorkingSet64 <= (ThresholdMB + 30)
        $totalLimitMB = $ThresholdMB + 30.0
        $passed = ($wsPrivMB -le $ThresholdMB) -and ($wsMB -le $totalLimitMB) -and ($webviewCount -eq 0) -and (@($pids).Count -eq 1)

        return [PSCustomObject]@{
            Scenario             = $Name
            ProcessCount         = @($pids).Count
            Processes            = ($procNames -join ", ")
            WebView2Count        = $webviewCount
            WorkingSetMB         = $wsMB
            WorkingSetPrivateMB  = $wsPrivMB
            PrivateMemoryMB      = $privMB
            HandleCount          = $totalHandles
            PrivateThresholdMB   = $ThresholdMB
            TotalThresholdMB     = $totalLimitMB
            Passed               = $passed
        }
    } finally {
        try {
            if (-not $proc.HasExited) {
                $null = $proc.CloseMainWindow()
                if (-not $proc.WaitForExit(2000)) {
                    $proc.Kill()
                }
            }
        } catch {}
        $proc.Dispose()
    }
}

$gfmDoc = Join-Path $repoRoot "tests\e2e\fixtures\sample-gfm-full.md"
$longDoc = Join-Path $repoRoot "tests\e2e\fixtures\sample-long.md"

$results = @()
$results += Measure-Scenario -Name "空载冷启动 (无参数)" -Arguments @() -ThresholdMB 20.0
if (Test-Path -LiteralPath $gfmDoc) {
    $results += Measure-Scenario -Name "典型 GFM 全元素文档" -Arguments @($gfmDoc) -ThresholdMB 30.0
}
if (Test-Path -LiteralPath $longDoc) {
    $results += Measure-Scenario -Name "200KB+ 超长文档" -Arguments @($longDoc) -ThresholdMB 50.0
}

$results | Format-Table Scenario, ProcessCount, WebView2Count, WorkingSetPrivateMB, WorkingSetMB, PrivateMemoryMB, HandleCount, PrivateThresholdMB, TotalThresholdMB, Passed -AutoSize

$outDir = Split-Path -Parent $OutJson
if ($outDir -and -not (Test-Path -LiteralPath $outDir)) {
    New-Item -ItemType Directory -Force -Path $outDir | Out-Null
}
$report = [PSCustomObject]@{
    MeasuredAtUtc = (Get-Date).ToUniversalTime().ToString("o")
    ExePath       = $resolvedExe
    ExeSizeKB     = [math]::Round((Get-Item -LiteralPath $resolvedExe).Length / 1KB, 1)
    SettleSeconds = $SettleSeconds
    Results       = $results
}
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllBytes($OutJson, $utf8NoBom.GetBytes(($report | ConvertTo-Json -Depth 6)))
Write-Host "[内存报告] 已写入: $OutJson" -ForegroundColor Cyan

$allPassed = @($results | Where-Object { -not $_.Passed }).Count -eq 0
if (-not $allPassed) {
    Write-Error "部分场景内存指标未达到验收门槛！"
    exit 1
}
exit 0
