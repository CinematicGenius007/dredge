<#
.SYNOPSIS
  Rebuild and reinstall dredge, even while agent sessions have dredge.exe open.

.DESCRIPTION
  Windows cannot overwrite a running exe, so a plain `cargo install` fails with "Access is
  denied" while any `dredge mcp` process exists. By default this script renames the running exe
  out of the way and installs the new one; running sessions are not interrupted and keep the old
  code until they end.

  With -Stop it ends every running dredge process first. That disconnects the MCP server in
  every session that uses it, and those sessions have to be restarted (or reconnect their MCP
  server) to get dredge back. Use it only when no session is mid-task.

.EXAMPLE
  .\scripts\update.ps1          # safe: rename, then install
  .\scripts\update.ps1 -Stop    # end all dredge processes, then install
  .\scripts\update.ps1 -Pull    # git pull --ff-only first
#>
[CmdletBinding()]
param(
    [switch]$Stop,
    [switch]$Pull
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $env:USERPROFILE ".cargo\bin"
$exe = Join-Path $bin "dredge.exe"
$env:PATH = "$bin;$env:PATH"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo not found. Install Rust first (see docs/INSTALL.md)."
}

# Leftovers from earlier upgrades; skips any still in use.
Get-ChildItem $bin -Filter "dredge.old*.exe" -ErrorAction SilentlyContinue | ForEach-Object {
    try { Remove-Item $_.FullName -ErrorAction Stop } catch {}
}

if ($Pull) {
    git -C $repo pull --ff-only
    if ($LASTEXITCODE -ne 0) { throw "git pull failed" }
}

$running = @(Get-Process dredge -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) {
    if ($Stop) {
        Write-Host "Stopping $($running.Count) dredge process(es)..."
        $running | Stop-Process -Force
        Start-Sleep -Milliseconds 500
    } elseif (Test-Path $exe) {
        $old = Join-Path $bin ("dredge.old.{0}.exe" -f (Get-Date -Format "HHmmss"))
        Write-Host "$($running.Count) dredge process(es) running; moving the old exe aside."
        Rename-Item $exe $old
    }
}

Push-Location $repo
try {
    cargo install --path crates\dredge-cli
    if ($LASTEXITCODE -ne 0) { throw "cargo install failed" }
} finally {
    Pop-Location
}

& $exe --version
if ($running.Count -gt 0 -and -not $Stop) {
    Write-Host "Running sessions keep the old version until they end; new sessions use the new one."
}
if ($Stop -and $running.Count -gt 0) {
    Write-Host "Restart the agent sessions (or reconnect their dredge MCP server) to use dredge again."
}
