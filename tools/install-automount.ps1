# Registers apfs-mount --watch to run (hidden, elevated) at every logon,
# giving plug-and-play APFS mounting: insert a Mac drive, get a drive
# letter; unplug it, the letter disappears.
#
# Run from an elevated PowerShell in the repo root:
#   .\tools\install-automount.ps1
# Remove with:
#   .\tools\install-automount.ps1 -Uninstall

param([switch]$Uninstall)

$taskName = "APFS Auto-Mount"

if ($Uninstall) {
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    Write-Host "removed scheduled task '$taskName'"
    exit 0
}

$exe = Join-Path $PSScriptRoot "..\crates\apfs-winfsp\target\release\apfs-mount.exe"
$exe = (Resolve-Path $exe).Path
if (-not (Test-Path $exe)) {
    Write-Error "apfs-mount.exe not found — build it first: cargo build --release --manifest-path crates\apfs-winfsp\Cargo.toml"
    exit 1
}

$action = New-ScheduledTaskAction -Execute $exe -Argument "--watch"
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit ([TimeSpan]::Zero) -Hidden

Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger `
    -Principal $principal -Settings $settings -Force | Out-Null

Write-Host "installed scheduled task '$taskName' (runs elevated at logon)."
Write-Host "start it now without relogging:  Start-ScheduledTask -TaskName '$taskName'"
