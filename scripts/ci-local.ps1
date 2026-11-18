# Local CI for Rico, on the Windows PC: what GitHub Actions used to do
# (.github/workflows/ci.yml, installer.yml, security.yml), run here instead.
#
#   scripts\ci-local.ps1                                  checks + installer (before every merge)
#   scripts\ci-local.ps1 -Publish                         patch release (0.x.PATCH) from main
#   scripts\ci-local.ps1 -Publish -Candidate -Confirmed   candidate of a patch, when the owner asks for one
#   scripts\ci-local.ps1 -Publish -Candidate -Confirmed   feature release candidate (v0.X.0-rc.N)
#   scripts\ci-local.ps1 -Publish -Confirmed              feature release (v0.X.0), after its candidates
#
# (run with `powershell -ExecutionPolicy Bypass -File scripts\ci-local.ps1 …`)
#
# Steps:
#   1. quality: cargo fmt --check, clippy -D warnings, tests, release build (--locked);
#   2. security, like the "Sicurezza" workflow: cargo audit, cargo deny,
#      Semgrep, Gitleaks (whole history); the reports are judged by
#      .github/scripts/security_report.py, so the same findings fail here as
#      on GitHub (report in target\ci-local\security-report.md);
#   3. installer: Inno Setup with the version from Cargo.toml;
#   4. -Publish, only from a clean `main` equal to origin/main: tag and
#      GitHub release with the installer, its SHA-256 and the CHANGELOG
#      section. A feature version (x.y.0) is published only with -Confirmed,
#      which is passed only after the owner confirmed that candidate (or the
#      final release): see AGENTS.md, rule 9.
#
# Every step writes its output to target\ci-local\; the summary at the end
# says what passed, what failed and what was not run.

param(
    [switch]$Publish,
    [switch]$Candidate,
    [switch]$Confirmed,
    [switch]$SkipSecurity
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$logs = Join-Path $root 'target\ci-local'
$reports = Join-Path $logs 'report'
Remove-Item -Recurse -Force $reports -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $reports | Out-Null
$results = [System.Collections.Generic.List[object]]::new()

# Tools installed with winget or pip after this shell started.
$env:Path = @(
    [Environment]::GetEnvironmentVariable('Path', 'User'),
    [Environment]::GetEnvironmentVariable('Path', 'Machine'),
    "$env:APPDATA\Python\Python312\Scripts"
) -join ';'
$env:PYTHONUTF8 = '1'

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
$version = ($meta.packages | Where-Object name -eq 'rico').version
$commit = (git rev-parse HEAD).Trim()
$branch = (git rev-parse --abbrev-ref HEAD).Trim()
Write-Host "Rico $version, $branch @ $($commit.Substring(0, 8))"

# A candidate is built as <version>-rc.N: the program (RICO_PRERELEASE) and
# the installer know it is older than its final release (in-app updates).
$full = $version
Remove-Item Env:RICO_PRERELEASE -ErrorAction SilentlyContinue
if ($Publish -and $Candidate) {
    $n = 1
    while (git ls-remote --tags origin "refs/tags/v$version-rc.$n") { $n++ }
    $full = "$version-rc.$n"
    $env:RICO_PRERELEASE = "rc.$n"
    Write-Host "Candidate build: $full"
}

# The release build replaces target\release\rico.exe: it must not be running.
if (Get-Process rico -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$root*" }) {
    throw 'Rico (from this folder) is running: close it first.'
}

# 1. Quality
$ok = Invoke-Step 'fmt' 'cargo' @('fmt', '--check')
$ok = (Invoke-Step 'clippy' 'cargo' @('clippy', '--all-targets', '--locked', '--', '-D', 'warnings')) -and $ok
$ok = (Invoke-Step 'test' 'cargo' @('test', '--locked')) -and $ok
$ok = (Invoke-Step 'build' 'cargo' @('build', '--release', '--locked', '--bin', 'rico', '--bin', 'rico-tui', '--bin', 'rico-sign')) -and $ok

# 2. Security: the same tools and the same judgement as the "Sicurezza" workflow.
# The tools themselves "fail" when they find something; what counts is the
# report script's verdict.
$utf8 = New-Object System.Text.UTF8Encoding $false
function Save-Report([string]$Step, [string]$File, [string[]]$Arguments, [string]$Out, [switch]$FromStderr) {
    Write-Host "==> $Step" -ForegroundColor Cyan
    $ErrorActionPreference = 'Continue'
    if ($FromStderr) {
        # cargo deny writes its JSON diagnostics on stderr.
        [string[]]$text = & $File @Arguments 2>&1 | Where-Object { $_ -is [System.Management.Automation.ErrorRecord] } |
            ForEach-Object { $_.ToString() }
    } else {
        [string[]]$text = & $File @Arguments 2> (Join-Path $logs "$Step.log")
    }
    $ErrorActionPreference = 'Stop'
    # JSON without a byte order mark: the report script reads plain UTF-8.
    if ($Out) { [System.IO.File]::WriteAllLines((Join-Path $reports $Out), [string[]]@($text), $utf8) }
}

if ($SkipSecurity) {
    Add-Result 'security' 'NOT RUN' '-SkipSecurity'
} else {
    $python = Find-Tool @("$env:LOCALAPPDATA\Programs\Python\Python312\python.exe", 'python3', 'python')
    $semgrep = Find-Tool @('semgrep')
    $gitleaks = Find-Tool @('gitleaks')
    Save-Report 'cargo-audit' 'cargo' @('audit', '--json') 'audit.json'
    Save-Report 'cargo-deny' 'cargo' @('deny', '--format', 'json', 'check', 'bans', 'licenses', 'sources') 'deny.jsonl' -FromStderr
    if ($semgrep) {
        Save-Report 'semgrep' $semgrep @('scan', '--metrics=off', '--quiet', '--config', 'p/rust', '--config', 'p/secrets',
            '--config', 'p/github-actions', '--json', '--output', (Join-Path $reports 'semgrep.json'), '.') $null
    }
    if ($gitleaks) {
        Save-Report 'gitleaks' $gitleaks @('git', '.', '--redact', '--no-banner', '--exit-code', '0',
            '--report-format', 'json', '--report-path', (Join-Path $reports 'gitleaks.json')) $null
    }
    if (-not $python) {
        Add-Result 'security' 'NOT RUN' 'Python not installed (needed for the report)'
        $ok = $false
    } else {
        $outputs = Join-Path $logs 'security-outputs.txt'
        Remove-Item $outputs -ErrorAction SilentlyContinue
        $env:GITHUB_OUTPUT = $outputs
        $env:REF = $branch
        $env:SHA = $commit
        $env:JOBS_OK = 'true'
        $md = Join-Path $logs 'security-report.md'
        & $python .github/scripts/security_report.py $reports | Set-Content -Encoding utf8 $md
        Remove-Item Env:GITHUB_OUTPUT
        $alarm = (Get-Content $outputs -ErrorAction SilentlyContinue) -contains 'importanti=true'
        if ($alarm) {
            Get-Content $md | Write-Host
            Add-Result 'security' 'FAILED' "important findings, see $md"
            $ok = $false
        } else {
            Add-Result 'security' 'ok' "minor warnings in $md"
        }
        if (-not $semgrep) { Add-Result 'semgrep' 'NOT RUN' 'not installed (counted as a missing check)' }
        if (-not $gitleaks) { Add-Result 'gitleaks' 'NOT RUN' 'not installed (counted as a missing check)' }
    }
}

# 3. Installer
$setup = $null
$hash = $null
if ($ok) {
    $iscc = Find-Tool @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", 'ISCC')
    if (-not $iscc) {
        Add-Result 'installer' 'NOT RUN' 'Inno Setup not installed'
        $ok = $false
    } elseif (Invoke-Step 'installer' $iscc @('/Q', "/DAppVersion=$full", 'installer\rico.iss')) {
        $setup = Get-Item "installer\Output\rico-setup-$full.exe"
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
    $feature = $version -match '^\d+\.\d+\.0$'
    $tag = "v$version"
    $why = $null
    $gh = Find-Tool @('gh', "$env:ProgramFiles\GitHub CLI\gh.exe")
    if (-not $ok) { $why = 'earlier steps failed' }
    elseif ($Candidate -and -not $Confirmed) { $why = 'a candidate is published only after the owner asked for it (-Confirmed)' }
    elseif ($feature -and -not $Confirmed) { $why = "$version is a feature release: publish only after the owner confirmed it (-Confirmed)" }
    elseif ($branch -ne 'main') { $why = "on branch $branch, releases come only from main" }
    elseif (git status --porcelain --untracked-files=no) { $why = 'uncommitted changes' }
    elseif (-not $gh) { $why = 'GitHub CLI (gh) not installed' }
    else {
        & $gh auth status *> $null
        if ($LASTEXITCODE -ne 0) { $why = 'GitHub CLI not signed in: run `gh auth login`' }
        else {
            git fetch -q --tags origin main
            if ((git rev-parse origin/main).Trim() -ne $commit) { $why = 'main differs from origin/main: push or pull first' }
            elseif (git ls-remote --tags origin "refs/tags/$tag") { $why = "tag $tag already exists: bump the version" }
        }
    }
    if (-not $why -and $Candidate) {
        # The candidate number chosen before the build.
        $tag = "v$full"
    }
    if ($why) {
        Add-Result 'release' 'NOT RUN' $why
        $ok = $false
    } else {
        # Release notes: this version's CHANGELOG section, the checksum, the warnings.
        # CHANGELOG.md is UTF-8: Windows PowerShell would read it as ANSI.
        [string[]]$lines = Get-Content -Encoding UTF8 CHANGELOG.md
        $start = [array]::FindIndex($lines, [Predicate[string]] { param($l) $l -like "## $version*" })
        $section = @()
        if ($start -ge 0 -and $start + 1 -lt $lines.Count) {
            [string[]]$section = $lines[($start + 1)..($lines.Count - 1)]
            $end = [array]::FindIndex($section, [Predicate[string]] { param($l) $l -like '## *' })
            if ($end -gt 0) { $section = $section[0..($end - 1)] }
        }
        $kind = if ($Candidate) { 'Release candidate, for testing before the final release. ' } else { '' }
        $notes = Join-Path $logs "release-notes-$tag.md"
        [string[]]$body = @(
            "$($kind)Windows installer built and checked on the owner's PC (scripts/ci-local.ps1) from ``main`` ($($commit.Substring(0, 7))). Not signed: Windows SmartScreen may ask for confirmation on the first start."
            ''
            "SHA-256 of ``$($setup.Name)``:"
            '```'
            $hash
            '```'
            ''
            "Video from Go Live streams is decoded with OpenH264 built from source: Cisco's H.264 patent licence does not cover it (see README)."
            ''
            "### Changes in $version"
        ) + $section
        # UTF-8 without a byte order mark (it would show up in the notes).
        [System.IO.File]::WriteAllLines($notes, $body, $utf8)
        # In-app updates: the signed manifest, when this build has the
        # owner's public keys (rico-sign status: 0 = yes, 3 = no keys yet).
        # The everyday key comes from the system keychain (rico-sign keygen).
        $assets = @($setup.FullName)
        $signer = Join-Path $root 'target\release\rico-sign.exe'
        & $signer status *> $null
        $keys = $LASTEXITCODE
        if ($keys -eq 0) {
            $manifest = Join-Path $logs 'rico-update.json'
            Remove-Item "$manifest*" -ErrorAction SilentlyContinue
            $channel = if ($Candidate) { 'candidate' } else { 'stable' }
            $signed = (Invoke-Step 'manifest' $signer @('manifest', $setup.FullName, '--version', $full,
                    '--channel', $channel, '--notes', "https://github.com/Trive253x/rico/releases/tag/$tag",
                    '--out', $manifest)) -and
                (Invoke-Step 'sign' $signer @('sign', $manifest)) -and
                (Invoke-Step 'verify' $signer @('verify', $manifest))
            if ($signed) { $assets += @($manifest, "$manifest.sig") } else { $ok = $false }
        } elseif ($keys -eq 3) {
            Add-Result 'manifest' 'NOT RUN' 'no public keys in this build yet: no in-app update for this release'
        } else {
            Add-Result 'manifest' 'FAILED' 'rico-sign status failed'
            $ok = $false
        }
        if ($ok) {
            git tag -a $tag -m "Rico $($tag.TrimStart('v'))" $commit
            git push -q origin $tag
            $ghArgs = @('release', 'create', $tag) + $assets + @('--verify-tag', '--title', "Rico $($tag.TrimStart('v'))", '--notes-file', $notes)
            if ($Candidate) { $ghArgs += '--prerelease' } else { $ghArgs += '--latest' }
            $ok = (Invoke-Step 'release' $gh $ghArgs) -and $ok
            if ($ok) { $results[$results.Count - 1].Note = $tag }
        } else {
            Add-Result 'release' 'NOT RUN' 'the update manifest could not be made'
        }
    }
}

Write-Host ''
$results | Format-Table -AutoSize | Out-String -Width 200 | Write-Host
Write-Host 'Not covered locally: builds and tests on Linux and macOS, demo screenshots.'
if ($ok) { Write-Host 'LOCAL CI: OK' -ForegroundColor Green; exit 0 }
Write-Host 'LOCAL CI: FAILED' -ForegroundColor Red
exit 1
