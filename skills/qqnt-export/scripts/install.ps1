param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'qqnt-export\bin'),
    [switch]$AddToUserPath,
    [string]$Repository = 'Spreng624/qqnt-export'
)

$ErrorActionPreference = 'Stop'
$releaseBase = "https://github.com/$Repository/releases/latest/download"
$executable = Join-Path $InstallDir 'qqnt-export.exe'
$temporary = Join-Path ([IO.Path]::GetTempPath()) ("qqnt-export-install-" + [guid]::NewGuid())

function Add-InstallToUserPath {
    param([string]$Directory)

    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    $parts = @($current -split ';' | Where-Object { $_ })
    if ($parts -notcontains $Directory) {
        [Environment]::SetEnvironmentVariable('Path', (@($parts) + $Directory) -join ';', 'User')
        Write-Host '已将安装目录加入当前用户的 PATH；新开终端后生效。'
    }
}

function Download-File {
    param([string]$Url, [string]$Destination)

    if (Get-Command curl.exe -ErrorAction SilentlyContinue) {
        & curl.exe -L --fail --retry 3 --connect-timeout 15 --max-time 300 `
            --silent --show-error -o $Destination $Url
        if ($LASTEXITCODE -ne 0) {
            throw "下载失败: $Url"
        }
        return
    }

    Invoke-WebRequest -Uri $Url -OutFile $Destination -UseBasicParsing
}

New-Item -ItemType Directory -Force -Path $temporary | Out-Null

try {
    $checksumFile = Join-Path $temporary 'SHA256SUMS.txt'
    Download-File "$releaseBase/SHA256SUMS.txt" $checksumFile
    $entry = Get-Content -LiteralPath $checksumFile |
        Where-Object { $_ -match 'qqnt-export\.exe\s*$' } |
        Select-Object -First 1
    if (-not $entry) {
        throw 'SHA256SUMS.txt 中没有 qqnt-export.exe'
    }
    $expected = ($entry -split '\s+')[0].ToLowerInvariant()

    if (Test-Path -LiteralPath $executable) {
        $current = (Get-FileHash -Algorithm SHA256 -LiteralPath $executable).Hash.ToLowerInvariant()
        if ($current -eq $expected) {
            Write-Host "QQNT Export 已是最新版本: $executable"
            if ($AddToUserPath) {
                Add-InstallToUserPath $InstallDir
            }
            return
        }
    }

    $download = Join-Path $temporary 'qqnt-export.exe'
    Download-File "$releaseBase/qqnt-export.exe" $download
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $download).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "SHA-256 校验失败: 期望 $expected，实际 $actual"
    }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $incoming = "$executable.incoming"
    Copy-Item -LiteralPath $download -Destination $incoming -Force
    Move-Item -LiteralPath $incoming -Destination $executable -Force

    if ($AddToUserPath) {
        Add-InstallToUserPath $InstallDir
    }

    Write-Host "QQNT Export 已安装: $executable"
    Write-Host "运行: & `"$executable`" -h"
}
finally {
    Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
}
