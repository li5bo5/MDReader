#Requires -Version 5.1
<#
.SYNOPSIS
    在当前用户（HKCU）下注册 MDReader 为 .md / .markdown 文件的打开方式（无需管理员权限）。
.DESCRIPTION
    默认自动定位仓库内的 target\release\MDReader.exe 或同目录 MDReader.exe，也可通过 -ExePath 显式指定。
    通过 Start-Process -Wait -PassThru 同步调用目标 GUI 程序内建的 `--register` 命令并校验退出码。
.PARAMETER ExePath
    要关联的 MDReader.exe 路径（可选）。
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

if (-not $ExePath -or -not (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
    Write-Error "未找到可用的 MDReader.exe，请先执行 cargo build --release 或通过 -ExePath 指定路径。"
    exit 1
}

$resolvedExe = (Resolve-Path -LiteralPath $ExePath).Path
$proc = Start-Process -FilePath $resolvedExe -ArgumentList "--register" -Wait -PassThru -WindowStyle Hidden
$code = $proc.ExitCode
if ($code -ne 0) {
    Write-Error "执行 `"$resolvedExe`" --register 失败，退出码: $code"
    exit $code
}

Write-Host "[MDReader] 已成功在 HKCU 注册 .md / .markdown 文件打开方式 -> $resolvedExe" -ForegroundColor Green
exit 0
