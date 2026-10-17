param(
    [Parameter(Position = 0)]
    [ValidateSet("start", "stop", "restart", "status")]
    [string]$Action = "start"
)

$ErrorActionPreference = "Stop"
$Root = $PSScriptRoot
$RunDir = if ($env:PIECEOFF_RUN_DIR) { $env:PIECEOFF_RUN_DIR } else { Join-Path $Root ".pieceoff-runtime" }
$DataDir = if ($env:PIECEOFF_DATA_DIR) { $env:PIECEOFF_DATA_DIR } else { Join-Path $Root "pieceoff-data" }

function Resolve-Binaries([bool]$Build = $false) {
    if ($env:PIECEOFF_BIN_DIR) {
        $binDir = $env:PIECEOFF_BIN_DIR
    } elseif ((Test-Path (Join-Path $Root "pieceoff-server.exe")) -and (Test-Path (Join-Path $Root "pieceoff-app.exe"))) {
        $binDir = $Root
    } else {
        $cargo = Get-Command cargo -ErrorAction SilentlyContinue
        if ($Build -and $cargo) {
            Write-Host "Building PieceOff..."
            & cargo build --workspace --manifest-path (Join-Path $Root "Cargo.toml")
            if ($LASTEXITCODE -ne 0) { throw "Cargo build failed with exit code $LASTEXITCODE." }
            $binDir = Join-Path $Root "target\debug"
        } else {
            $binDir = Join-Path $Root "target\debug"
        }
    }

    $script:ServerBin = Join-Path $binDir "pieceoff-server.exe"
    $script:AppBin = Join-Path $binDir "pieceoff-app.exe"
    if ($Build) {
        foreach ($binary in @($script:ServerBin, $script:AppBin)) {
            if (!(Test-Path $binary -PathType Leaf)) { throw "PieceOff executable not found: $binary. Install Rust/Cargo or set PIECEOFF_BIN_DIR." }
        }
    }
}

function Get-ManagedProcess([string]$PidFile, [string]$ExpectedPath) {
    if (!(Test-Path $PidFile)) { return $null }
    $rawPid = (Get-Content $PidFile -Raw).Trim()
    $pidValue = 0
    if (!([int]::TryParse($rawPid, [ref]$pidValue))) { return $null }
    $process = Get-Process -Id $pidValue -ErrorAction SilentlyContinue
    if (!$process) { return $null }
    try {
        if ([IO.Path]::GetFullPath($process.Path) -ieq [IO.Path]::GetFullPath($ExpectedPath)) { return $process }
    } catch { }
    return $null
}

function Start-ManagedProcess([string]$Label, [string]$Binary, [string]$PidFile, [string]$LogFile, [bool]$HideWindow) {
    $existing = Get-ManagedProcess $PidFile $Binary
    if ($existing) { Write-Host "$Label is already running (PID $($existing.Id))."; return }
    Remove-Item $PidFile -Force -ErrorAction SilentlyContinue
    $arguments = @{
        FilePath = $Binary
        WorkingDirectory = $Root
        PassThru = $true
        RedirectStandardOutput = $LogFile
        RedirectStandardError = "$LogFile.err"
    }
    if ($HideWindow) { $arguments.WindowStyle = "Hidden" }
    $process = Start-Process @arguments
    Set-Content -Path $PidFile -Value $process.Id -NoNewline
    Start-Sleep -Seconds 1
    $process.Refresh()
    if ($process.HasExited -or !(Get-ManagedProcess $PidFile $Binary)) {
        Remove-Item $PidFile -Force -ErrorAction SilentlyContinue
        Write-Host "$Label did not stay running. Recent log output:" -ForegroundColor Red
        if (Test-Path $LogFile) { Get-Content $LogFile -Tail 30 }
        if (Test-Path "$LogFile.err") { Get-Content "$LogFile.err" -Tail 30 }
        throw "$Label failed to start."
    }
    Write-Host "Started $Label (PID $($process.Id)). Log: $LogFile"
}

function Stop-ManagedProcess([string]$Label, [string]$PidFile, [string]$Binary) {
    $process = Get-ManagedProcess $PidFile $Binary
    if (!$process) {
        Remove-Item $PidFile -Force -ErrorAction SilentlyContinue
        Write-Host "$Label is not running."
        return
    }
    Write-Host "Stopping $Label (PID $($process.Id))..."
    if ($process.MainWindowHandle -ne 0) { [void]$process.CloseMainWindow() }
    if (!$process.WaitForExit(5000)) {
        Stop-Process -Id $process.Id -Force
        [void]$process.WaitForExit(5000)
    }
    Remove-Item $PidFile -Force -ErrorAction SilentlyContinue
}

function Show-Status {
    foreach ($entry in @(
        @{ Label = "Room server"; PidFile = (Join-Path $RunDir "server.pid"); Binary = $script:ServerBin },
        @{ Label = "Desktop app"; PidFile = (Join-Path $RunDir "app.pid"); Binary = $script:AppBin }
    )) {
        $process = Get-ManagedProcess $entry.PidFile $entry.Binary
        if ($process) { Write-Host "$($entry.Label) is running (PID $($process.Id))." }
        else { Write-Host "$($entry.Label) is stopped." }
    }
}

function Start-PieceOff {
    Resolve-Binaries $true
    New-Item -ItemType Directory -Force -Path $RunDir, $DataDir | Out-Null
    $bind = if ($env:PIECEOFF_BIND) { $env:PIECEOFF_BIND } else { "0.0.0.0:28878" }
    if (!$env:PIECEOFF_SERVER_URL) {
        $port = $bind.Substring($bind.LastIndexOf(":") + 1)
        $env:PIECEOFF_SERVER_URL = "ws://127.0.0.1:$port/ws"
    }
    $env:PIECEOFF_BIND = $bind
    $env:PIECEOFF_DATA_DIR = $DataDir
    Write-Host "PieceOff data: $DataDir"
    Start-ManagedProcess "room server" $script:ServerBin (Join-Path $RunDir "server.pid") (Join-Path $RunDir "server.log") $true
    Start-ManagedProcess "desktop app" $script:AppBin (Join-Path $RunDir "app.pid") (Join-Path $RunDir "app.log") $false
    Write-Host "Open the app and create a room. Run '.\pieceoff.ps1 stop' to close PieceOff."
}

New-Item -ItemType Directory -Force -Path $RunDir | Out-Null
switch ($Action) {
    "start" { Start-PieceOff }
    "stop" {
        Resolve-Binaries $false
        Stop-ManagedProcess "desktop app" (Join-Path $RunDir "app.pid") $script:AppBin
        Stop-ManagedProcess "room server" (Join-Path $RunDir "server.pid") $script:ServerBin
    }
    "restart" {
        Resolve-Binaries $false
        Stop-ManagedProcess "desktop app" (Join-Path $RunDir "app.pid") $script:AppBin
        Stop-ManagedProcess "room server" (Join-Path $RunDir "server.pid") $script:ServerBin
        Start-PieceOff
    }
    "status" { Resolve-Binaries $false; Show-Status }
}
