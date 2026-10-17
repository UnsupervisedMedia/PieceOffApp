# Assemble a Windows release bundle: the app and server, license notices,
# and docs, packed into dist\<name>.zip. FFmpeg is excluded by default.
#
#   .\scripts\bundle.ps1 [-Target win64|winarm64] [-WithFfmpeg] [-SkipBuild] [-SkipVerify]
#
# Not yet exercised on a Windows machine: run it on one before relying on it.
param(
    [ValidateSet("win64", "winarm64")]
    [string]$Target = "win64",
    [switch]$WithFfmpeg,
    [switch]$SkipBuild,
    [switch]$SkipVerify
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

$Version = (Select-String -Path "crates/pieceoff-app/Cargo.toml" -Pattern '^version = "(.*)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
$Name = "PieceOff-$Version-$Target"
$Dist = Join-Path $Root "dist"
$Out = Join-Path $Dist $Name

if (-not $SkipBuild) {
    & cargo build --release --locked -p pieceoff-app -p pieceoff-server
    if ($LASTEXITCODE -ne 0) { throw "Cargo build failed with exit code $LASTEXITCODE." }
}
foreach ($binary in @("pieceoff-app.exe", "pieceoff-server.exe")) {
    if (-not (Test-Path (Join-Path $Root "target\release\$binary"))) {
        throw "target\release\$binary is missing; build first."
    }
}

if (Test-Path $Out) { Remove-Item -Recurse $Out }
New-Item -ItemType Directory -Force -Path (Join-Path $Out "docs") | Out-Null
Copy-Item target\release\pieceoff-app.exe, target\release\pieceoff-server.exe, pieceoff.ps1, README.md, LICENSE, CONTRIBUTING.md $Out
foreach ($doc in @("PACKAGING.md", "FFMPEG.md", "TESTING.md", "DEPLOYMENT.md", "LICENSING.md")) {
    $path = Join-Path "docs" $doc
    if (Test-Path $path) { Copy-Item $path (Join-Path $Out "docs") }
}

if ($WithFfmpeg) {
    if (-not $env:PIECEOFF_FFMPEG_SOURCE_ARCHIVE -or -not (Test-Path $env:PIECEOFF_FFMPEG_SOURCE_ARCHIVE)) {
        throw "FFmpeg redistribution requires PIECEOFF_FFMPEG_SOURCE_ARCHIVE containing the complete corresponding source and build materials for this exact pin. See docs/FFMPEG.md; omit -WithFfmpeg to require users to install FFmpeg separately."
    }
    $sourceOut = Join-Path $Out "source"
    New-Item -ItemType Directory -Force -Path $sourceOut | Out-Null
    $sourceExtension = if ($env:PIECEOFF_FFMPEG_SOURCE_ARCHIVE -match '\.tar\.xz$') { ".tar.xz" } elseif ($env:PIECEOFF_FFMPEG_SOURCE_ARCHIVE -match '\.tar\.gz$') { ".tar.gz" } else { ".zip" }
    $sourceName = "FFmpeg-corresponding-source" + $sourceExtension
    Copy-Item $env:PIECEOFF_FFMPEG_SOURCE_ARCHIVE (Join-Path $sourceOut $sourceName)
    $sourceHash = (Get-FileHash (Join-Path $sourceOut $sourceName) -Algorithm SHA256).Hash.ToLower()
    "$sourceHash  $sourceName" | Set-Content (Join-Path $sourceOut "FFmpeg-corresponding-source.sha256")
    $pin = Get-Content "packaging/ffmpeg-pins.txt" |
        Where-Object { $_ -match "^$Target " } | Select-Object -First 1
    if (-not $pin) { throw "No pinned FFmpeg build for $Target (see docs/FFMPEG.md)." }
    $parts = $pin -split " "
    $expected = $parts[1]
    $url = $parts[2]
    $archiveName = Split-Path -Leaf $url
    $cache = if ($env:PIECEOFF_DOWNLOAD_CACHE) { $env:PIECEOFF_DOWNLOAD_CACHE } else { Join-Path $Dist ".cache" }
    New-Item -ItemType Directory -Force -Path $cache | Out-Null
    $archive = Join-Path $cache $archiveName
    $needsDownload = $true
    if (Test-Path $archive) {
        $needsDownload = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLower() -ne $expected
    }
    if ($needsDownload) {
        Write-Host "Downloading $archiveName"
        Invoke-WebRequest -Uri $url -OutFile "$archive.part"
        Move-Item -Force "$archive.part" $archive
    }
    $actual = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLower()
    if ($actual -ne $expected) {
        Remove-Item $archive
        throw "SHA-256 mismatch for $archiveName`n  expected $expected`n  actual   $actual"
    }
    $work = Join-Path ([System.IO.Path]::GetTempPath()) ("pieceoff-ffmpeg-" + [guid]::NewGuid())
    Expand-Archive -Path $archive -DestinationPath $work
    $src = Get-ChildItem $work -Directory | Select-Object -First 1
    $ffmpegOut = Join-Path $Out "ffmpeg"
    New-Item -ItemType Directory -Force -Path $ffmpegOut | Out-Null
    Copy-Item (Join-Path $src.FullName "bin\ffmpeg.exe"), (Join-Path $src.FullName "bin\ffprobe.exe") $ffmpegOut
    Copy-Item (Join-Path $src.FullName "LICENSE.txt") (Join-Path $ffmpegOut "LICENSE-FFmpeg.txt")
    Remove-Item -Recurse $work
    @"
This folder holds an unmodified FFmpeg build, run by PieceOff as a separate
program (it is not linked into PieceOff).

Build:    $archiveName
Archive:  $url
SHA-256:  $expected
License:  GNU LGPL version 3 (see LICENSE-FFmpeg.txt)
Built by: https://github.com/BtbN/FFmpeg-Builds (its build scripts and the
          exact configuration are public in that repository)
Source:   FFmpeg is free software; its source code is available from
          https://ffmpeg.org/download.html and https://git.ffmpeg.org/ffmpeg.git
          (this build is the commit named in the archive file name).
          See ../source/FFmpeg-corresponding-source.* in this bundle for the
          exact corresponding source and build materials.

You can replace these programs with any other FFmpeg build: put your own
ffmpeg.exe and ffprobe.exe in this folder, or choose a folder in PieceOff
under Media tools.
"@ | Set-Content (Join-Path $ffmpegOut "FFMPEG-BUILD.txt")
}

& python scripts/license-report.py (Join-Path $Out "THIRD_PARTY_LICENSES.txt")
if ($LASTEXITCODE -ne 0) { throw "The license report failed." }
& python scripts/third-party-notices.py (Join-Path $Out "THIRD_PARTY_NOTICES.txt")
if ($LASTEXITCODE -ne 0) { throw "The third-party notices generation failed." }

if (-not $SkipVerify) {
    Write-Host "Verifying the bundle..."
    if ($WithFfmpeg) {
        & (Join-Path $Out "ffmpeg\ffmpeg.exe") -version | Select-Object -First 1
        & (Join-Path $Out "ffmpeg\ffprobe.exe") -version | Select-Object -First 1
    }
    $port = if ($env:PIECEOFF_VERIFY_PORT) { $env:PIECEOFF_VERIFY_PORT } else { "28979" }
    $data = Join-Path ([System.IO.Path]::GetTempPath()) ("pieceoff-verify-" + [guid]::NewGuid())
    New-Item -ItemType Directory -Force -Path $data | Out-Null
    $env:PIECEOFF_BIND = "127.0.0.1:$port"
    $env:PIECEOFF_DATA_DIR = $data
    $server = Start-Process -FilePath (Join-Path $Out "pieceoff-server.exe") -PassThru -WindowStyle Hidden
    $healthy = $false
    for ($i = 0; $i -lt 10 -and -not $healthy; $i++) {
        try {
            Invoke-WebRequest -Uri "http://127.0.0.1:$port/health" -UseBasicParsing | Out-Null
            $healthy = $true
        } catch { Start-Sleep -Milliseconds 500 }
    }
    Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
    if (-not $healthy) { throw "The bundled server did not answer /health." }
    Write-Host "Bundled server answers /health."
}

$zip = Join-Path $Dist "$Name.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path $Out -DestinationPath $zip
$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
"$hash  $Name.zip" | Set-Content (Join-Path $Dist "$Name.zip.sha256")
Write-Host "Bundle:   dist\$Name.zip"
Write-Host "Checksum: dist\$Name.zip.sha256"
