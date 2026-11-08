# Local CI for ferrocord, on the Windows PC: what GitHub Actions used to do
# (.github/workflows/ci.yml, installer.yml, security.yml), run here instead.
#
#   powershell -ExecutionPolicy Bypass -File scripts\ci-local.ps1            # checks + installer
#   powershell -ExecutionPolicy Bypass -File scripts\ci-local.ps1 -Publish   # also the GitHub release
#
# Steps:
#   1. quality: cargo fmt --check, clippy -D warnings, tests, release build (--locked);
#   2. security: cargo audit (fails on vulnerabilities not excepted in
#      .cargo/audit.toml), cargo deny (bans, licenses, sources); gitleaks and
#      semgrep when installed, otherwise reported as not run;
#   3. installer: Inno Setup with the version from Cargo.toml;
#   4. with -Publish, only from a clean `main` equal to origin/main: tag
#      v<version> and GitHub release with the installer and its SHA-256
#      (needs the GitHub CLI, `gh auth login` done once by the owner).
#
# Every step writes its output to target\ci-local\; the summary at the end
# says what passed, what failed and what was not run.

param(
    [switch]$Publish,
    [switch]$SkipSecurity
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$logs = Join-Path $root 'target\ci-local'
New-Item -ItemType Directory -Force $logs | Out-Null
$results = [System.Collections.Generic.List[object]]::new()

function Add-Result([string]$Step, [string]$State, [string]$Note = '') {
    $results.Add([pscustomobject]@{ Step = $Step; State = $State; Note = $Note })
}

# Runs a native command, saves its output, records the result. $true if it passed.
function Invoke-Step([string]$Step, [string]$File, [string[]]$Arguments) {
    $log = Join-Path $logs (($Step -replace '[^\w]+', '-') + '.log')
    Write-Host "==> $Step" -ForegroundColor Cyan
    $ErrorActionPreference = 'Continue'
    & $File @Arguments *> $log
    $code = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($code -eq 0) {
        Add-Result $Step 'ok'
        return $true
    }
    Get-Content $log | Select-Object -Last 30 | Write-Host
    Add-Result $Step 'FAILED' "exit $code, see $log"
    return $false
}

function Find-Tool([string[]]$Candidates) {
    foreach ($c in $Candidates) {
        if ($c -and (Test-Path $c)) { return $c }
        $cmd = Get-Command $c -ErrorAction SilentlyContinue
        if ($cmd) { return $cmd.Source }
    }
    return $null
}

# libopus is built with CMake: the one from the Visual Studio Build Tools.
if (-not $env:CMAKE) {
    $vsCmake = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe'
    if (Test-Path $vsCmake) { $env:CMAKE = $vsCmake }
}

$meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
$version = ($meta.packages | Where-Object name -eq 'ferrocord').version
$commit = (git rev-parse HEAD).Trim()
$branch = (git rev-parse --abbrev-ref HEAD).Trim()
Write-Host "ferrocord $version, $branch @ $($commit.Substring(0, 8))"

# The release build replaces target\release\ferrocord.exe: it must not be running.
if (Get-Process ferrocord -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$root*" }) {
    throw 'ferrocord (from this folder) is running: close it first.'
}

# 1. Quality
$ok = Invoke-Step 'fmt' 'cargo' @('fmt', '--check')
$ok = (Invoke-Step 'clippy' 'cargo' @('clippy', '--all-targets', '--locked', '--', '-D', 'warnings')) -and $ok
$ok = (Invoke-Step 'test' 'cargo' @('test', '--locked')) -and $ok
$ok = (Invoke-Step 'build' 'cargo' @('build', '--release', '--locked', '--bin', 'ferrocord', '--bin', 'ferrocord-tui')) -and $ok

# 2. Security
if ($SkipSecurity) {
    Add-Result 'security' 'NOT RUN' '-SkipSecurity'
} else {
    $ok = (Invoke-Step 'cargo audit' 'cargo' @('audit')) -and $ok
    $ok = (Invoke-Step 'cargo deny' 'cargo' @('deny', 'check', 'bans', 'licenses', 'sources')) -and $ok
    $gitleaks = Find-Tool @('gitleaks')
    if ($gitleaks) {
        $ok = (Invoke-Step 'gitleaks' $gitleaks @('git', '.', '--redact', '--no-banner')) -and $ok
    } else {
        Add-Result 'gitleaks' 'NOT RUN' 'not installed'
    }
    $semgrep = Find-Tool @('semgrep')
    if ($semgrep) {
        $ok = (Invoke-Step 'semgrep' $semgrep @('scan', '--metrics=off', '--quiet', '--error', '--config', 'p/rust', '--config', 'p/secrets', '.')) -and $ok
    } else {
        Add-Result 'semgrep' 'NOT RUN' 'not installed'
    }
}

# 3. Installer
$setup = $null
if ($ok) {
    $iscc = Find-Tool @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", 'ISCC')
    if (-not $iscc) {
        Add-Result 'installer' 'NOT RUN' 'Inno Setup not installed'
        $ok = $false
    } elseif (Invoke-Step 'installer' $iscc @('/Q', "/DAppVersion=$version", 'installer\ferrocord.iss')) {
        $setup = Get-Item "installer\Output\ferrocord-setup-$version.exe"
        $hash = (Get-FileHash $setup.FullName -Algorithm SHA256).Hash.ToLower()
        $results[$results.Count - 1].Note = "$($setup.Name), SHA-256 $hash"
    } else {
        $ok = $false
    }
} else {
    Add-Result 'installer' 'NOT RUN' 'earlier steps failed'
}

# 4. Release
if ($Publish) {
    $why = $null
    $gh = Find-Tool @('gh', "$env:ProgramFiles\GitHub CLI\gh.exe")
    if (-not $ok) { $why = 'earlier steps failed' }
    elseif ($branch -ne 'main') { $why = "on branch $branch, releases come only from main" }
    elseif (git status --porcelain --untracked-files=no) { $why = 'uncommitted changes' }
    elseif (-not $gh) { $why = 'GitHub CLI (gh) not installed' }
    else {
        git fetch -q origin main
        if ((git rev-parse origin/main).Trim() -ne $commit) { $why = 'main differs from origin/main: push or pull first' }
        elseif (git ls-remote --tags origin "refs/tags/v$version") { $why = "tag v$version already exists: bump the version" }
    }
    if ($why) {
        Add-Result 'release' 'NOT RUN' $why
        $ok = $false
    } else {
        # Release notes: this version's CHANGELOG section, the checksum, the warnings.
        [string[]]$lines = Get-Content CHANGELOG.md
        $start = [array]::FindIndex($lines, [Predicate[string]] { param($l) $l -like "## $version*" })
        $section = @()
        if ($start -ge 0) {
            $section = $lines[($start + 1)..($lines.Count - 1)]
            $end = [array]::FindIndex([string[]]$section, [Predicate[string]] { param($l) $l -like '## *' })
            if ($end -ge 0) { $section = $section[0..($end - 1)] }
        }
        $notes = Join-Path $logs "release-notes-$version.md"
        @(
            "Windows installer built and checked on the owner's PC (scripts/ci-local.ps1) from ``main`` ($($commit.Substring(0, 7))). Not signed: Windows SmartScreen may ask for confirmation on the first start."
            ''
            "SHA-256 of ``$($setup.Name)``:"
            '```'
            $hash
            '```'
            ''
            "Video from Go Live streams is decoded with OpenH264 built from source: Cisco's H.264 patent licence does not cover it (see README)."
            ''
            "### Changes in $version"
        ) + $section | Set-Content -Encoding utf8 $notes
        git tag -a "v$version" -m "ferrocord $version" $commit
        git push -q origin "v$version"
        $published = Invoke-Step 'release' $gh @('release', 'create', "v$version", $setup.FullName, '--verify-tag', '--title', "ferrocord $version", '--notes-file', $notes, '--latest')
        $ok = $published -and $ok
    }
}

Write-Host ''
$results | Format-Table -AutoSize | Out-String -Width 200 | Write-Host
Write-Host 'Not covered locally: builds and tests on Linux and macOS, demo screenshots.'
if ($ok) { Write-Host 'LOCAL CI: OK' -ForegroundColor Green; exit 0 }
Write-Host 'LOCAL CI: FAILED' -ForegroundColor Red
exit 1
