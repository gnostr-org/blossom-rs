<#
.SYNOPSIS
    Install and manage a GitHub Actions self-hosted runner on Windows.

.DESCRIPTION
    Downloads, configures, and registers a self-hosted runner as a Windows
    service. Supports org-scoped and repo-scoped runners. Requires the gh CLI
    authenticated with an account that has org/repo admin rights.

.PARAMETER Action
    install | start | stop | status | uninstall | remove  (default: install)

.PARAMETER Org
    GitHub organization (default: MonumentalSystems, env: RUNNER_ORG)

.PARAMETER Repo
    Scope runner to a single repo. Runner dir becomes $Home\actions-runner\<repo>.
    Uses repo-level token API (requires repo admin rights).

.PARAMETER Name
    Runner name (default: <hostname>-windows)

.PARAMETER Labels
    Comma-separated runner labels (default: self-hosted,Windows,X64,windows-latest)

.PARAMETER Dir
    Runner install directory (default: $Home\actions-runner or $Home\actions-runner\<repo>)

.PARAMETER Group
    Runner group (default: Default)

.PARAMETER Remove
    Remove existing runner config before installing

.EXAMPLE
    .\scripts\windows-runner.ps1 install
    .\scripts\windows-runner.ps1 -Org my-org install
    .\scripts\windows-runner.ps1 -Org my-org -Name my-runner status
    .\scripts\windows-runner.ps1 -Remove install
    .\scripts\windows-runner.ps1 -Repo blossom-rs install
#>

[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('install','start','stop','status','uninstall','remove')]
    [string]$Action = 'install',

    [string]$Org    = ($env:RUNNER_ORG  ?? 'MonumentalSystems'),
    [string]$Repo   = ($env:RUNNER_REPO ?? ''),
    [string]$Name   = ($env:RUNNER_NAME ?? "$env:COMPUTERNAME-windows"),
    [string]$Labels = 'self-hosted,Windows,X64,windows-latest',
    [string]$Dir    = '',
    [string]$Group  = 'Default',
    [switch]$Remove
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ── helpers ───────────────────────────────────────────────────────────────────
function Info  { param([string]$Msg) Write-Host "[info]  $Msg" }
function Warn  { param([string]$Msg) Write-Host "[warn]  $Msg" -ForegroundColor Yellow }
function Err   { param([string]$Msg) Write-Error "[error] $Msg" }

function Require {
    param([string]$Cmd)
    if (-not (Get-Command $Cmd -ErrorAction SilentlyContinue)) {
        Err "'$Cmd' is required but not found in PATH."
    }
}

# Animated trailing dots while a ScriptBlock runs in a background job.
function Invoke-WithDots {
    param([string]$Label, [scriptblock]$ScriptBlock)
    $base = $Label.TrimEnd('.')
    $job  = Start-Job -ScriptBlock $ScriptBlock
    $dots = @('.', '..', '...')
    $i    = 0
    while ($job.State -eq 'Running') {
        Write-Host -NoNewline ("`r[info]  $base" + $dots[$i % 3].PadRight(3) + '   ')
        $i++
        Start-Sleep -Milliseconds 300
    }
    $null = Receive-Job $job -Wait -AutoRemoveJob -ErrorVariable jobErr 2>&1
    if ($jobErr) {
        Write-Host "`r[error] $base  ✗" -ForegroundColor Red
        Write-Error ($jobErr -join "`n")
    } else {
        Write-Host "`r[info]  $base  ✓"
    }
}

# ── scope helpers ─────────────────────────────────────────────────────────────
function ScopeLabel {
    if ($Repo) { "repo: $Org/$Repo" } else { "org: $Org" }
}

function RunnerUrl {
    if ($Repo) { "https://github.com/$Org/$Repo" } else { "https://github.com/$Org" }
}

function RunnersSettingsUrl {
    if ($Repo) { "https://github.com/$Org/$Repo/settings/actions/runners" }
    else       { "https://github.com/organizations/$Org/settings/actions/runners" }
}

function RegistrationToken {
    if ($Repo) {
        gh api "repos/$Org/$Repo/actions/runners/registration-token" --method POST --jq '.token'
    } else {
        gh api "orgs/$Org/actions/runners/registration-token" --method POST --jq '.token'
    }
}

function RemovalToken {
    if ($Repo) {
        gh api "repos/$Org/$Repo/actions/runners/remove-token" --method POST --jq '.token'
    } else {
        gh api "orgs/$Org/actions/runners/remove-token" --method POST --jq '.token'
    }
}

function LatestRunnerInfo {
    # Returns hashtable with Url and Sha256
    $release = Invoke-RestMethod 'https://api.github.com/repos/actions/runner/releases/latest'
    $asset   = $release.assets | Where-Object { $_.name -like '*win-x64*.zip' } | Select-Object -First 1
    if (-not $asset) { Err 'Could not find win-x64 runner asset in latest release.' }
    $sha = ($asset.digest -replace '^sha256:', '')
    @{ Url = $asset.browser_download_url; Sha256 = $sha }
}

function Verify-Archive {
    param([string]$File, [string]$Expected)
    if (-not $Expected) {
        Warn "No SHA256 from API — skipping verification"
        return
    }
    $actual = (Get-FileHash -Path $File -Algorithm SHA256).Hash.ToLower()
    Info "SHA256 expected: $Expected"
    Info "SHA256 actual:   $actual"
    if ($actual -eq $Expected) {
        Info "SHA256 verified ✓"
    } else {
        Err "SHA256 mismatch!"
    }
}

# ── resolve runner dir ────────────────────────────────────────────────────────
if (-not $Dir) {
    $Dir = if ($Repo) { Join-Path $HOME "actions-runner\$Repo" }
           else       { Join-Path $HOME 'actions-runner' }
}

# ── commands ──────────────────────────────────────────────────────────────────
function Cmd-Remove {
    $cfg = Join-Path $Dir 'config.cmd'
    if (-not (Test-Path $cfg)) {
        Info "No runner config found at $Dir — nothing to remove."
        return
    }
    Require gh
    Info "Fetching removal token ($(ScopeLabel))"
    $token = RemovalToken
    Info "Removing runner config from GitHub"
    & $cfg remove --token $token
}

function Cmd-Install {
    Require gh

    if ($Remove) { Cmd-Remove }

    Info "Fetching registration token ($(ScopeLabel))"
    $token = RegistrationToken

    Info "Resolving latest runner release..."
    $info    = LatestRunnerInfo
    $url     = $info.Url
    $sha     = $info.Sha256
    $version = [regex]::Match($url, '\d+\.\d+\.\d+').Value
    Info "Runner version: $version"

    Info "Installing runner to $Dir"
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null

    $archive       = Join-Path $Dir 'runner.zip'
    $parentArchive = Join-Path (Split-Path $Dir -Parent) 'runner.zip'

    if ($Repo -and (Test-Path $parentArchive)) {
        Warn "Reusing existing archive from parent: $parentArchive"
        Warn "If the runner version is stale, delete the cached archive and re-run:"
        Warn "  Remove-Item '$parentArchive'"
        Copy-Item $parentArchive $archive -Force
    } else {
        Info "Downloading runner archive..."
        if ($Repo) {
            Invoke-WebRequest -Uri $url -OutFile $parentArchive -UseBasicParsing
            Copy-Item $parentArchive $archive -Force
        } else {
            Invoke-WebRequest -Uri $url -OutFile $archive -UseBasicParsing
        }
    }
    Verify-Archive -File $archive -Expected $sha

    Invoke-WithDots "Extracting runner archive..." {
        Expand-Archive -Path $using:archive -DestinationPath $using:Dir -Force
    }

    $configCmd = Join-Path $Dir 'config.cmd'
    Invoke-WithDots "Configuring runner..." {
        & $using:configCmd `
            --url (& { if ($using:Repo) { "https://github.com/$using:Org/$using:Repo" } else { "https://github.com/$using:Org" } }) `
            --token $using:token `
            --name $using:Name `
            --labels $using:Labels `
            --runnergroup $using:Group `
            --work (Join-Path $using:Dir '_work') `
            --unattended `
            --replace
    }

    $svcCmd = Join-Path $Dir 'svc.cmd'
    # Remove stale service if present
    & $svcCmd stop      2>$null; $true
    & $svcCmd uninstall 2>$null; $true
    Invoke-WithDots "Installing Windows service..." { & $using:svcCmd install }
    Invoke-WithDots "Starting service..."           { & $using:svcCmd start   }

    Info "Done. Runner '$Name' is registered and running."
    Info "View at: $(RunnersSettingsUrl)"
}

function Cmd-Start {
    Info "Starting runner service"
    & (Join-Path $Dir 'svc.cmd') start
}

function Cmd-Stop {
    Info "Stopping runner service"
    & (Join-Path $Dir 'svc.cmd') stop
}

function Cmd-Status {
    & (Join-Path $Dir 'svc.cmd') status
}

function Cmd-Uninstall {
    Require gh
    $svcCmd = Join-Path $Dir 'svc.cmd'
    Info "Stopping and removing service"
    & $svcCmd stop      2>$null; $true
    & $svcCmd uninstall 2>$null; $true

    Info "Fetching removal token ($(ScopeLabel))"
    $token = RemovalToken
    Info "Removing runner from GitHub"
    & (Join-Path $Dir 'config.cmd') remove --token $token

    Info "Deleting $Dir"
    Remove-Item -Recurse -Force $Dir
    Info "Runner uninstalled."
}

# ── entrypoint ────────────────────────────────────────────────────────────────
switch ($Action) {
    'install'   { Cmd-Install   }
    'remove'    { Cmd-Remove    }
    'start'     { Cmd-Start     }
    'stop'      { Cmd-Stop      }
    'status'    { Cmd-Status    }
    'uninstall' { Cmd-Uninstall }
}
