# APFS for Windows — installer.
#
# Run elevated from an extracted release zip:
#   .\install.ps1              # install + register auto-mount at logon
#   .\install.ps1 -Uninstall
#
# What it does:
#   1. copies apfs.exe / apfs-mount.exe to %LOCALAPPDATA%\Programs\APFS
#   2. adds that directory to the user PATH
#   3. registers the "APFS Auto-Mount" logon task (elevated, hidden)
#      running `apfs-mount --watch` from the INSTALLED location
#   4. starts it immediately
#
# From then on the installed copy keeps itself current: the watcher checks
# GitHub Releases daily and `apfs-mount update` forces a check manually.
# WinFsp (https://winfsp.dev) must be installed once, separately.

param([switch]$Uninstall)

$ErrorActionPreference = "Stop"
$installDir = Join-Path $env:LOCALAPPDATA "Programs\APFS"
$taskName = "APFS Auto-Mount"

function Remove-FromUserPath([string]$dir) {
    $path = [Environment]::GetEnvironmentVariable("Path", "User")
    $clean = ($path -split ";" | Where-Object { $_ -and $_ -ne $dir }) -join ";"
    [Environment]::SetEnvironmentVariable("Path", $clean, "User")
}

if ($Uninstall) {
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    Stop-Process -Name "apfs-mount" -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $installDir -ErrorAction SilentlyContinue
    Remove-FromUserPath $installDir
    Write-Host "uninstalled."
    exit 0
}

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error "run this installer from an elevated PowerShell (auto-mount needs raw disk access)."
    exit 1
}

# 1. Copy binaries next to this script into the install dir.
New-Item -ItemType Directory -Force -Path $installDir | Out-Null
Stop-Process -Name "apfs-mount" -Force -ErrorAction SilentlyContinue
Copy-Item (Join-Path $PSScriptRoot "apfs.exe") $installDir -Force
Copy-Item (Join-Path $PSScriptRoot "apfs-mount.exe") $installDir -Force

# 2. User PATH so `apfs` / `apfs-mount` work in any prompt.
$path = [Environment]::GetEnvironmentVariable("Path", "User")
if (($path -split ";") -notcontains $installDir) {
    [Environment]::SetEnvironmentVariable("Path", "$path;$installDir", "User")
}

# 3. Logon task running the installed watcher.
$exe = Join-Path $installDir "apfs-mount.exe"
$action = New-ScheduledTaskAction -Execute $exe -Argument "--watch"
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$taskPrincipal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit ([TimeSpan]::Zero) -Hidden
Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger `
    -Principal $taskPrincipal -Settings $settings -Force | Out-Null

# 4. Go.
Start-ScheduledTask -TaskName $taskName

Write-Host "installed to $installDir"
Write-Host "auto-mount is running now and at every logon; plug in an APFS drive to test."
Write-Host "updates: automatic (daily check), or run:  apfs-mount update"
