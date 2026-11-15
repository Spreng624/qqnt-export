param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'qqnt-export\bin'),
    [switch]$AddToUserPath,
    [string]$Repository = 'Spreng624/qqnt-export'
)

$ErrorActionPreference = 'Stop'
$releaseBase = "https://github.com/$Repository/releases/latest/download"
$executable = Join-Path $InstallDir 'qqnt-export.exe'
$temporary = Join-Path ([IO.Path]::GetTempPath()) ("qqnt-export-install-" + [guid]::NewGuid())
$releaseAssets = $null

function Add-InstallToUserPath {
    param([string]$Directory)

    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    $parts = @($current -split ';' | Where-Object { $_ })
    if ($parts -notcontains $Directory) {
        [Environment]::SetEnvironmentVariable('Path', (@($parts) + $Directory) -join ';', 'User')
        Write-Host 'Added the install directory to the user PATH. Open a new terminal to use it.'
    }
}

function Download-File {
    param([string]$Url, [string]$Destination)

    if (Get-Command curl.exe -ErrorAction SilentlyContinue) {
        $curlArgs = @(
            '-L', '--fail', '--retry', '3',
            '--connect-timeout', '15', '--max-time', '300',
            '--silent', '--show-error', '-o', $Destination, $Url
        )
        & curl.exe @curlArgs
        if ($LASTEXITCODE -ne 0) {
            throw "Download failed: $Url"
        }
        return
    }

    Invoke-WebRequest -Uri $Url -OutFile $Destination -UseBasicParsing
}

function Load-ReleaseAssets {
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    $release = Invoke-RestMethod `
        -Uri "https://api.github.com/repos/$Repository/releases/latest" `
        -Headers @{ Accept = 'application/vnd.github+json'; 'User-Agent' = 'qqnt-export-skill' } `
        -UseBasicParsing
    $script:releaseAssets = @($release.assets)
}

function Download-ReleaseAsset {
    param([string]$Name, [string]$Destination)

    if ($script:releaseAssets) {
        $asset = $script:releaseAssets | Where-Object { $_.name -eq $Name } | Select-Object -First 1
        if (-not $asset) {
            throw "Release asset not found: $Name"
        }
        if (Get-Command curl.exe -ErrorAction SilentlyContinue) {
            & curl.exe -L --fail --retry 5 --retry-delay 2 `
                --connect-timeout 15 --max-time 300 --silent --show-error `
                -H 'Accept: application/octet-stream' -o $Destination $asset.url
            if ($LASTEXITCODE -ne 0) {
                throw "Download failed: $($asset.url)"
            }
            return
        }
        Invoke-WebRequest `
            -Uri $asset.url `
            -Headers @{ Accept = 'application/octet-stream'; 'User-Agent' = 'qqnt-export-skill' } `
            -OutFile $Destination `
            -UseBasicParsing
        return
    }

    Download-File "$releaseBase/$Name" $Destination
}

New-Item -ItemType Directory -Force -Path $temporary | Out-Null

try {
    $checksumFile = Join-Path $temporary 'SHA256SUMS.txt'
    try {
        Download-ReleaseAsset 'SHA256SUMS.txt' $checksumFile
    }
    catch {
        try {
            Load-ReleaseAssets
            Download-ReleaseAsset 'SHA256SUMS.txt' $checksumFile
        }
        catch {
            if (Test-Path -LiteralPath $executable) {
                Write-Host "Update check failed; using the installed version: $executable"
                return
            }
            throw
        }
    }
    $entry = Get-Content -LiteralPath $checksumFile |
        Where-Object { $_ -match 'qqnt-export\.exe\s*$' } |
        Select-Object -First 1
    if (-not $entry) {
        throw 'SHA256SUMS.txt does not contain qqnt-export.exe'
    }
    $expected = ($entry -split '\s+')[0].ToLowerInvariant()

    if (Test-Path -LiteralPath $executable) {
        $current = (Get-FileHash -Algorithm SHA256 -LiteralPath $executable).Hash.ToLowerInvariant()
        if ($current -eq $expected) {
            Write-Host "QQNT Export is up to date: $executable"
            if ($AddToUserPath) {
                Add-InstallToUserPath $InstallDir
            }
            return
        }
    }

    $download = Join-Path $temporary 'qqnt-export.exe'
    try {
        Download-ReleaseAsset 'qqnt-export.exe' $download
    }
    catch {
        Load-ReleaseAssets
        Download-ReleaseAsset 'qqnt-export.exe' $download
    }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $download).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "SHA-256 mismatch: expected $expected, got $actual"
    }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $incoming = "$executable.incoming"
    Copy-Item -LiteralPath $download -Destination $incoming -Force
    Move-Item -LiteralPath $incoming -Destination $executable -Force

    if ($AddToUserPath) {
        Add-InstallToUserPath $InstallDir
    }

    Write-Host "QQNT Export installed: $executable"
    Write-Host "Run: & `"$executable`" -h"
}
finally {
    Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
}
