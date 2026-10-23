# Assemble a Windows release bundle: the app and server, FFmpeg with its
# corresponding source, license notices, and docs, packed into dist\<name>.zip.
#
#   .\scripts\bundle.ps1 [-FfmpegStage <dir>] [-NoFfmpeg] [-SkipBuild] [-SkipVerify]
#
# FFmpeg is built by scripts/build-ffmpeg.sh, which needs MSYS2 (MINGW64 shell,
# with gcc, make, nasm, diffutils). Run it there first:
#   scripts/build-ffmpeg.sh win64 dist/ffmpeg-stage
# and pass that folder as -FfmpegStage (or set PIECEOFF_FFMPEG_STAGE).
param(
    [string]$FfmpegStage = $env:PIECEOFF_FFMPEG_STAGE,
    [switch]$NoFfmpeg,
    [switch]$SkipBuild,
    [switch]$SkipVerify
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

$Version = (Select-String -Path "crates/pieceoff-app/Cargo.toml" -Pattern '^version = "(.*)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
$Target = "win64"
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

if (-not $NoFfmpeg) {
    if (-not $FfmpegStage -or -not (Test-Path (Join-Path $FfmpegStage "ffmpeg\ffmpeg.exe"))) {
        throw "No FFmpeg build to bundle. Build it in an MSYS2 MINGW64 shell with scripts/build-ffmpeg.sh win64 <dir>, then pass -FfmpegStage <dir> (see docs/FFMPEG.md), or pass -NoFfmpeg."
    }
    Copy-Item -Recurse (Join-Path $FfmpegStage "ffmpeg") (Join-Path $Out "ffmpeg")
    Copy-Item -Recurse (Join-Path $FfmpegStage "source") (Join-Path $Out "source")
}

& python scripts/license-report.py (Join-Path $Out "THIRD_PARTY_LICENSES.txt")
if ($LASTEXITCODE -ne 0) { throw "The license report failed." }
& python scripts/third-party-notices.py (Join-Path $Out "THIRD_PARTY_NOTICES.txt")
if ($LASTEXITCODE -ne 0) { throw "The third-party notices generation failed." }

if (-not $SkipVerify) {
    Write-Host "Verifying the bundle..."
    if (-not $NoFfmpeg) {
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
