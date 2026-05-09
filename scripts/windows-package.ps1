Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$script:RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$script:WindowsToolchain = "stable-x86_64-pc-windows-gnu"
$script:WindowsDistName = "ytuff-windows-x64"
$script:RequiredWimgRuntime = @(
    "wimg.exe",
    "libgcc_s_seh-1.dll",
    "libjpeg-8.dll",
    "libpng16-16.dll",
    "libsixel-1.dll",
    "libwinpthread-1.dll",
    "zlib1.dll"
)
$script:RequiredPortableFiles = @(
    "ytuff.exe",
    "WebView2Loader.dll",
    "wimg.exe",
    "libgcc_s_seh-1.dll",
    "libjpeg-8.dll",
    "libpng16-16.dll",
    "libsixel-1.dll",
    "libwinpthread-1.dll",
    "zlib1.dll",
    "ffmpeg.exe",
    "ffprobe.exe",
    "install-user.ps1",
    "install-user.bat",
    "README.txt",
    "LICENSE"
)

function Get-YTuffVersion {
    $cargoToml = Join-Path $script:RepoRoot "Cargo.toml"
    $content = Get-Content $cargoToml -Raw
    if ($content -match '(?m)^version = "([^"]+)"') {
        return $Matches[1]
    }
    throw "Could not determine package version from $cargoToml"
}

function Get-WindowsReleaseDir {
    return Join-Path $script:RepoRoot "target\release"
}

function Get-WindowsStageDir {
    return Join-Path $script:RepoRoot "dist\$script:WindowsDistName"
}

function Get-WindowsZipPath {
    return Join-Path $script:RepoRoot "dist\$script:WindowsDistName.zip"
}

function Get-WindowsMsiPath {
    return Join-Path $script:RepoRoot "dist\$script:WindowsDistName.msi"
}

function Invoke-YTuffShutdown {
    try {
        & taskkill /IM ytuff.exe /F 2>$null | Out-Null
    } catch {
    }
}

function Find-WebView2Loader {
    $buildDir = Join-Path (Get-WindowsReleaseDir) "build"
    $loader = Get-ChildItem -Path $buildDir -Recurse -Filter "WebView2Loader.dll" -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -like "*\out\x64\WebView2Loader.dll" } |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1

    if ($null -eq $loader) {
        throw "Missing WebView2Loader.dll under $buildDir. Refusing to create a broken Windows package."
    }

    return $loader.FullName
}

function Resolve-WimgSourceDir {
    $candidates = @(
        $env:YTUFF_WIMG_DIR,
        (Join-Path $script:RepoRoot "wimg\build_wimg"),
        (Join-Path $script:RepoRoot ".tmp_wimg\wimg\build_wimg")
    ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }

    foreach ($candidate in $candidates) {
        if (-not (Test-Path $candidate)) {
            continue
        }

        $missing = @()
        foreach ($name in $script:RequiredWimgRuntime) {
            if (-not (Test-Path (Join-Path $candidate $name))) {
                $missing += $name
            }
        }

        if ($missing.Count -eq 0) {
            return (Resolve-Path $candidate).Path
        }
    }

    throw "Could not find a complete wimg runtime. Set YTUFF_WIMG_DIR or provide wimg\build_wimg with: $($script:RequiredWimgRuntime -join ', ')"
}

function Resolve-FfmpegSourceDir {
    $candidates = New-Object System.Collections.Generic.List[string]

    if (-not [string]::IsNullOrWhiteSpace($env:YTUFF_FFMPEG_DIR)) {
        $candidates.Add($env:YTUFF_FFMPEG_DIR)
    }

    foreach ($commandName in @("ffmpeg.exe", "ffprobe.exe")) {
        $command = Get-Command $commandName -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($null -ne $command) {
            $commandDir = Split-Path -Parent $command.Source
            if (-not [string]::IsNullOrWhiteSpace($commandDir)) {
                $candidates.Add($commandDir)
            }
        }
    }

    $candidates.Add((Join-Path $script:RepoRoot "dist\$script:WindowsDistName"))

    foreach ($candidate in $candidates) {
        if ([string]::IsNullOrWhiteSpace($candidate) -or -not (Test-Path $candidate)) {
            continue
        }

        $ffmpegPath = Join-Path $candidate "ffmpeg.exe"
        $ffprobePath = Join-Path $candidate "ffprobe.exe"
        if ((Test-Path $ffmpegPath) -and (Test-Path $ffprobePath)) {
            return (Resolve-Path $candidate).Path
        }
    }

    throw "Could not find ffmpeg.exe and ffprobe.exe together. Set YTUFF_FFMPEG_DIR or make them available on PATH."
}

function Write-PortableInstallScripts {
    param(
        [Parameter(Mandatory = $true)]
        [string]$StageDir
    )

    @'
$AppDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")

$parts = @()
if (-not [string]::IsNullOrWhiteSpace($UserPath)) {
    $parts = $UserPath -split ";" | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
}

$alreadyInstalled = $false
foreach ($part in $parts) {
    if ($part.TrimEnd("\") -ieq $AppDir.TrimEnd("\")) {
        $alreadyInstalled = $true
        break
    }
}

if ($alreadyInstalled) {
    Write-Host "YTuff is already in your user PATH."
} else {
    $newParts = @($parts + $AppDir)
    $newPath = ($newParts | Select-Object -Unique) -join ";"
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    Write-Host "YTuff was added to your user PATH."
}

Write-Host ""
Write-Host "Restart your terminal, then run:"
Write-Host "  ytuff tui"
Write-Host ""
Read-Host "Press Enter to close"
'@ | Set-Content -Path (Join-Path $StageDir "install-user.ps1") -Encoding UTF8

    @'
@echo off
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install-user.ps1"
'@ | Set-Content -Path (Join-Path $StageDir "install-user.bat") -Encoding ASCII

    @'
YTuff Windows x64

Run:
  ytuff.exe tui

To install ytuff into your user PATH:
  Double-click install-user.bat

After installing:
  Restart your terminal
  Run: ytuff tui

Do not delete these files:
  WebView2Loader.dll
  wimg.exe
  ffmpeg.exe
  ffprobe.exe
  *.dll
'@ | Set-Content -Path (Join-Path $StageDir "README.txt") -Encoding UTF8
}

function Assert-PortableStage {
    param(
        [Parameter(Mandatory = $true)]
        [string]$StageDir
    )

    $missing = @()
    foreach ($name in $script:RequiredPortableFiles) {
        if (-not (Test-Path (Join-Path $StageDir $name))) {
            $missing += $name
        }
    }

    if ($missing.Count -gt 0) {
        throw "The Windows package is incomplete. Missing: $($missing -join ', ')"
    }
}

function New-YTuffWindowsStage {
    param(
        [switch]$Clean
    )

    $version = Get-YTuffVersion
    $stageDir = Get-WindowsStageDir
    $releaseDir = Get-WindowsReleaseDir
    $wimgSourceDir = Resolve-WimgSourceDir
    $ffmpegSourceDir = Resolve-FfmpegSourceDir

    Write-Host "=== YTuff Windows Packager ===" -ForegroundColor Cyan
    Write-Host "Building version: $version" -ForegroundColor Yellow

    Invoke-YTuffShutdown

    if ($Clean) {
        Write-Host "[1/5] Cleaning..." -ForegroundColor Green
        & cargo clean | Out-Host
        if ($LASTEXITCODE -ne 0) {
            throw "cargo clean failed"
        }
    } else {
        Write-Host "[1/5] Skipping clean build..." -ForegroundColor Green
    }

    Write-Host "[2/5] Building release for Windows ($script:WindowsToolchain)..." -ForegroundColor Green
    & cargo "+$script:WindowsToolchain" build --release -j 1 | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed"
    }

    Write-Host "[3/5] Stripping binary..." -ForegroundColor Green
    $stripExe = "C:\Program Files\Git\usr\bin\strip.exe"
    if (Test-Path $stripExe) {
        & $stripExe (Join-Path $releaseDir "ytuff.exe") | Out-Host
    }

    Write-Host "[4/5] Staging release payload..." -ForegroundColor Green
    Remove-Item $stageDir -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $stageDir | Out-Null

    $mainBinary = Join-Path $releaseDir "ytuff.exe"
    if (-not (Test-Path $mainBinary)) {
        throw "Missing built executable: $mainBinary"
    }
    Copy-Item $mainBinary $stageDir -Force

    Copy-Item (Find-WebView2Loader) $stageDir -Force

    foreach ($name in $script:RequiredWimgRuntime) {
        Copy-Item (Join-Path $wimgSourceDir $name) $stageDir -Force
    }

    foreach ($name in @("ffmpeg.exe", "ffprobe.exe")) {
        Copy-Item (Join-Path $ffmpegSourceDir $name) $stageDir -Force
    }

    Write-PortableInstallScripts -StageDir $stageDir
    Copy-Item (Join-Path $script:RepoRoot "LICENSE") $stageDir -Force

    Assert-PortableStage -StageDir $stageDir

    Write-Host "[5/5] Windows payload is ready." -ForegroundColor Green
    return $stageDir
}

function New-YTuffWindowsZip {
    param(
        [Parameter(Mandatory = $true)]
        [string]$StageDir
    )

    $zipPath = Get-WindowsZipPath
    Remove-Item $zipPath -Force -ErrorAction SilentlyContinue
    Compress-Archive -Path (Join-Path $StageDir "*") -DestinationPath $zipPath -Force
    return $zipPath
}

function Get-WixToolsetDir {
    $candle = Get-Command "candle.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
    $light = Get-Command "light.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
    if (($null -ne $candle) -and ($null -ne $light)) {
        return (Split-Path -Parent $candle.Source)
    }

    $wixDir = Join-Path $script:RepoRoot "dist\wix314"
    if ((Test-Path (Join-Path $wixDir "candle.exe")) -and (Test-Path (Join-Path $wixDir "light.exe"))) {
        return $wixDir
    }

    $zipPath = Join-Path $script:RepoRoot "dist\wix314-binaries.zip"
    $downloadUrl = "https://github.com/wixtoolset/wix3/releases/download/wix314rtm/wix314-binaries.zip"

    Write-Host "Downloading WiX Toolset v3.14 binaries..." -ForegroundColor Yellow
    Invoke-WebRequest -Uri $downloadUrl -OutFile $zipPath

    Remove-Item $wixDir -Recurse -Force -ErrorAction SilentlyContinue
    Expand-Archive -Path $zipPath -DestinationPath $wixDir -Force

    if ((-not (Test-Path (Join-Path $wixDir "candle.exe"))) -or (-not (Test-Path (Join-Path $wixDir "light.exe")))) {
        throw "WiX Toolset download did not produce candle.exe and light.exe"
    }

    return $wixDir
}

function New-YTuffWindowsMsi {
    param(
        [Parameter(Mandatory = $true)]
        [string]$StageDir
    )

    $wixDir = Get-WixToolsetDir
    $version = Get-YTuffVersion
    $msiPath = Get-WindowsMsiPath
    $objDir = Join-Path $script:RepoRoot "dist\wixobj"
    $wxsPath = Join-Path $script:RepoRoot "packaging\wix\ytuff.wxs"

    if (-not (Test-Path $wxsPath)) {
        throw "Missing WiX source file: $wxsPath"
    }

    Remove-Item $objDir -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $objDir | Out-Null
    Remove-Item $msiPath -Force -ErrorAction SilentlyContinue

    & (Join-Path $wixDir "candle.exe") `
        -nologo `
        -arch x64 `
        "-dStageDir=$StageDir" `
        "-dProductVersion=$version" `
        -out (Join-Path $objDir "") `
        $wxsPath | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "WiX candle.exe failed"
    }

    $wixobjPath = Join-Path $objDir "ytuff.wixobj"
    & (Join-Path $wixDir "light.exe") `
        -nologo `
        -out $msiPath `
        $wixobjPath | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "WiX light.exe failed"
    }

    return $msiPath
}

function Get-MsiProperty {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [string]$Property
    )

    $installer = New-Object -ComObject WindowsInstaller.Installer
    $database = $installer.GetType().InvokeMember("OpenDatabase", "InvokeMethod", $null, $installer, @($Path, 0))
    $view = $database.OpenView("SELECT `Value` FROM `Property` WHERE `Property`='$Property'")
    $view.Execute()
    $record = $view.Fetch()
    if ($null -eq $record) {
        return $null
    }
    return $record.StringData(1)
}
