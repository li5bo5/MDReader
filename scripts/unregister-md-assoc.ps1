#Requires -Version 5.1
<#
.SYNOPSIS
    从当前用户（HKCU）下精准卸载 MDReader 的 .md / .markdown 文件关联（不误伤第三方软件关联）。
.DESCRIPTION
    若提供了 -ExePath 或能够定位到 MDReader.exe，则通过 Start-Process -Wait -PassThru 同步调用 `--unregister`；
    若可执行文件已被移除，则直接按相同边界精准清理 HKCU 中 MDReader 自建键值。
.PARAMETER ExePath
    可选的 MDReader.exe 路径。
#>

[CmdletBinding()]
param(
    [string]$ExePath = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptDir = $PSScriptRoot
$repoRoot = Split-Path -Parent $scriptDir

if (-not $ExePath) {
    $candidates = @(
        (Join-Path $repoRoot "target\release\MDReader.exe"),
        (Join-Path $repoRoot "MDReader.exe"),
        (Join-Path $repoRoot "target\debug\MDReader.exe")
    )
    foreach ($cand in $candidates) {
        if (Test-Path -LiteralPath $cand -PathType Leaf) {
            $ExePath = $cand
            break
        }
    }
}

if ($ExePath -and (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
    $resolvedExe = (Resolve-Path -LiteralPath $ExePath).Path
    $proc = Start-Process -FilePath $resolvedExe -ArgumentList "--unregister" -Wait -PassThru -WindowStyle Hidden
    $code = $proc.ExitCode
    if ($code -ne 0) {
        Write-Error "执行 `"$resolvedExe`" --unregister 失败，退出码: $code"
        exit $code
    }
    Write-Host "[MDReader] 已从 HKCU 精准移除 MDReader 文件关联。" -ForegroundColor Green
    exit 0
}

# 兜底：若二进制已删除，按同等精确边界清理 HKCU 自建项
$hkcu = [Microsoft.Win32.Registry]::CurrentUser
$hkcu.DeleteSubKeyTree("Software\Classes\MDReader.Document", $false)
$hkcu.DeleteSubKeyTree("Software\Classes\Applications\MDReader.exe", $false)
$hkcu.DeleteSubKeyTree("Software\MDReader", $false)

foreach ($ext in @(".md", ".markdown")) {
    $owp = $hkcu.OpenSubKey("Software\Classes\$ext\OpenWithProgids", $true)
    if ($null -ne $owp) {
        try { $owp.DeleteValue("MDReader.Document", $false) } finally { $owp.Dispose() }
    }
}

$regApps = $hkcu.OpenSubKey("Software\RegisteredApplications", $true)
if ($null -ne $regApps) {
    try { $regApps.DeleteValue("MDReader", $false) } finally { $regApps.Dispose() }
}

Write-Host "[MDReader] 已从 HKCU 精准移除 MDReader 文件关联（脚本直清模式）。" -ForegroundColor Green
exit 0
