# Installing and upgrading dredge (Windows)

All commands are PowerShell. If `cargo` is not found, open a new terminal so `PATH` picks up
`~\.cargo\bin`.

## First install

```powershell
cd C:\Users\ayush\personal_projects\dredge
cargo install --path crates\dredge-cli
dredge --version
```

This builds an optimized binary and puts `dredge.exe` in `~\.cargo\bin`.

Register it as an MCP server for Claude Code, with your focus roots:

```powershell
claude mcp add dredge --scope user -e "DREDGE_FOCUS_ROOTS=C:\server;C:\" -- "$env:USERPROFILE\.cargo\bin\dredge.exe" mcp
```

Scan the focus roots once so agents have data on their first call:

```powershell
$env:DREDGE_FOCUS_ROOTS = "C:\server;C:\"
dredge scan
```

## Upgrading (the "Access is denied" problem)

Every agent session that uses the MCP server runs its own `dredge.exe mcp` process from
`~\.cargo\bin\dredge.exe`. Windows cannot overwrite an exe that is running, so
`cargo install` fails at the last step:

```
error: failed to move `...\cargo-installXXXX\dredge.exe` to `...\.cargo\bin\dredge.exe`
Caused by:
  Access is denied. (os error 5)
```

This happens on **every** reinstall while any session that uses dredge is open. Pick one fix.

### The script (does Option A or B for you)

```powershell
cd C:\Users\ayush\personal_projects\dredge
.\scripts\update.ps1          # rename the running exe aside, then install (sessions keep running)
.\scripts\update.ps1 -Stop    # end every dredge process first, then install
.\scripts\update.ps1 -Pull    # git pull --ff-only first (combine with either)
```

`-Stop` disconnects the dredge MCP server in every session that uses it, and those sessions must
be restarted or reconnect the server to get dredge back. Use it only when no session is
mid-task. The default mode interrupts nothing. If PowerShell blocks the script, run it as
`powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\update.ps1`.

The two manual options below are what the script does.

### Option A: rename the running exe, then install (no sessions need to close)

Windows allows renaming a running exe, just not overwriting it.

```powershell
$bin = "$env:USERPROFILE\.cargo\bin"

# Remove leftovers from earlier upgrades (skips any still in use)
Get-ChildItem $bin -Filter "dredge.old*.exe" | ForEach-Object {
    try { Remove-Item $_.FullName -ErrorAction Stop } catch {}
}

# Move the running exe out of the way, then install the new one
Rename-Item "$bin\dredge.exe" "dredge.old.$(Get-Date -Format HHmmss).exe"
cd C:\Users\ayush\personal_projects\dredge
cargo install --path crates\dredge-cli
dredge --version
```

Sessions that were already running keep using the old code until they end. New sessions pick up
the new binary. The `dredge.old.*.exe` files are harmless; the cleanup step at the top removes
them on a later upgrade once those sessions have ended.

### Option B: end the sessions first

1. See what is running:
   ```powershell
   Get-Process dredge | Select-Object Id, StartTime, Path
   ```
2. Close the agent sessions that use dredge (or restart them), so their `dredge mcp` processes
   exit. Do not kill the processes by hand unless you know which session owns each one.
3. Run `cargo install --path crates\dredge-cli`.

### Check that it worked

```powershell
dredge --version
Get-Process dredge | Select-Object Id, StartTime, Path
```

A session started after the upgrade runs the new binary. A session started before it still runs
the old one.

## Cleaning up

```powershell
# Old renamed binaries (only the ones no session is using can be deleted)
Get-ChildItem "$env:USERPROFILE\.cargo\bin" -Filter "dredge.old*.exe" | Remove-Item

# Build output inside the repo (rebuilt on demand)
cd C:\Users\ayush\personal_projects\dredge
cargo clean
```

Snapshots live in `%LOCALAPPDATA%\dredge` (or `DREDGE_DATA_DIR` if set). Delete the `.dredge`
files there to force a fresh scan; dredge never deletes them itself.
