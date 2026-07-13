# One-command commit & push.
#
#   .\tools\push.ps1                          # uses the default message below
#   .\tools\push.ps1 "fix: short description" # custom commit message
#
# First run bootstraps everything: git init, initial commit, private GitHub
# repo via gh, push. Later runs just add/commit/push.

param(
    [Parameter(Position = 0)]
    [string]$Message
)

$ErrorActionPreference = "Stop"
$repoName = "APFSExplorerIntegration"

# Default message for the current build — update when landing a milestone.
$defaultMessage = @"
APFS read support for Windows: WinFsp mount, plug-and-play watch mode

- apfs-core: dependency-free safe-Rust APFS reader (checkpoints, omaps,
  B-trees, inodes, extents, symlinks, xattrs, GPT, space manager),
  Fletcher-64 verified, 16 tests against an apfsck-validated fixture
- apfs-cli: info/ls/cat/tree for images and raw devices
- apfs-fixture: apfsck-clean test image injector (seed of write support)
- apfs-winfsp: read-only Explorer mounts via WinFsp Mount Manager,
  --watch mode auto-mounts/unmounts APFS drives like native removables,
  real free-space reporting, install-automount.ps1 for logon startup
- verified end-to-end on a real 222GB Time Machine drive (1M+ files)
"@

if (-not $Message) { $Message = $defaultMessage }

Set-Location (Join-Path $PSScriptRoot "..")

if (-not (Test-Path ".git")) {
    Write-Host "== first run: initialising repository =="
    git init | Out-Null
    git add .
    git commit -m $Message
    gh repo create $repoName --private --source=. --remote=origin --push
    $url = gh repo view --json url -q .url
    Write-Host "created and pushed: $url"
    Write-Host "note: update 'repository' in Cargo.toml to $url"
    exit 0
}

git add .
$pending = git status --porcelain
if (-not $pending) {
    Write-Host "nothing to commit - working tree clean."
    exit 0
}
git commit -m $Message
git push
Write-Host "pushed."
