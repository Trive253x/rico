#!/usr/bin/env bash
# Local CI for Rico on Linux (cloud sessions, Linux PCs): steps 1 and 2 of
# scripts/ci-local.ps1, with the same commands and the same judgement of the
# security reports (.github/scripts/security_report.py).
#
#   scripts/ci-linux.sh                 quality + security
#   scripts/ci-linux.sh --skip-security quality only
#
# It does NOT replace scripts\ci-local.ps1 on the Windows PC (AGENTS.md,
# rule 9): no Windows build (audio, screen capture and their cfg(windows)
# code), no installer, no releases. It catches what only shows on Linux and
# lets a cloud session check its work before pushing.
#
# Tools: Rust (fmt, clippy), cargo-audit, cargo-deny, python3; Semgrep and
# Gitleaks if on PATH (or in $SEMGREP / $GITLEAKS). A missing tool is
# "NOT RUN", which never counts as passed: the result is then not OK.
#
# Output in target/ci-local/ (logs, report/, security-report.md); the last
# line is "LOCAL CI (Linux): OK" or "LOCAL CI (Linux): FAILED".

set -u
cd "$(dirname "$0")/.." || exit 2

skip_security=false
for arg in "$@"; do
    case "$arg" in
        --skip-security) skip_security=true ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

logs=target/ci-local
reports="$logs/report"
rm -rf "$reports"
mkdir -p "$reports"

results=()
ok=true
add() { results+=("$(printf '%-12s %-8s %s' "$1" "$2" "${3:-}")"); }

# Runs a command, saves its output, records the result.
step() {
    local name=$1
    shift
    local log="$logs/$name.log"
    echo "==> $name"
    if "$@" >"$log" 2>&1; then
        add "$name" ok
    else
        add "$name" FAILED "exit $?, see $log"
        ok=false
    fi
}

find_tool() {
    local override=$1 name=$2
    if [ -n "$override" ] && [ -x "$override" ]; then
        echo "$override"
    elif command -v "$name" >/dev/null 2>&1; then
        command -v "$name"
    elif [ -x "$HOME/.local/bin/$name" ]; then
        echo "$HOME/.local/bin/$name"
    fi
}

# 1. Quality
step fmt cargo fmt --check
step clippy cargo clippy --all-targets --locked -- -D warnings
step test cargo test --locked
step build cargo build --release --locked --bin rico --bin rico-tui

# 2. Security: the tools "fail" when they find something; what counts is the
# report script's verdict, as on the Windows PC.
if $skip_security; then
    add security "NOT RUN" "--skip-security"
    ok=false
else
    echo "==> security"
    semgrep=$(find_tool "${SEMGREP:-}" semgrep)
    gitleaks=$(find_tool "${GITLEAKS:-}" gitleaks)
    if cargo audit --version >/dev/null 2>&1; then
        cargo audit --json >"$reports/audit.json" 2>"$logs/cargo-audit.log"
    else
        add cargo-audit "NOT RUN" "not installed (cargo install --locked cargo-audit)"
        ok=false
    fi
    if cargo deny --version >/dev/null 2>&1; then
        # cargo deny writes its JSON diagnostics on stderr.
        cargo deny --format json check bans licenses sources 2>"$reports/deny.jsonl" >/dev/null
    else
        add cargo-deny "NOT RUN" "not installed (cargo install --locked cargo-deny)"
        ok=false
    fi
    if [ -n "$semgrep" ]; then
        "$semgrep" scan --metrics=off --quiet --config p/rust --config p/secrets \
            --config p/github-actions --json --output "$reports/semgrep.json" . >"$logs/semgrep.log" 2>&1
    else
        add semgrep "NOT RUN" "not installed (pip install --user semgrep)"
        ok=false
    fi
    if [ -n "$gitleaks" ]; then
        "$gitleaks" git . --redact --no-banner --exit-code 0 \
            --report-format json --report-path "$reports/gitleaks.json" >"$logs/gitleaks.log" 2>&1
    else
        add gitleaks "NOT RUN" "not installed (github.com/gitleaks/gitleaks releases)"
        ok=false
    fi
    if command -v python3 >/dev/null 2>&1; then
        outputs="$logs/security-outputs.txt"
        rm -f "$outputs"
        md="$logs/security-report.md"
        GITHUB_OUTPUT="$outputs" REF="$(git rev-parse --abbrev-ref HEAD)" SHA="$(git rev-parse HEAD)" \
            JOBS_OK=true python3 .github/scripts/security_report.py "$reports" >"$md"
        if grep -qx 'importanti=true' "$outputs" 2>/dev/null; then
            cat "$md"
            add security FAILED "important findings, see $md"
            ok=false
        else
            add security ok "minor warnings in $md"
        fi
    else
        add security "NOT RUN" "python3 not installed (needed for the report)"
        ok=false
    fi
fi

add installer "NOT RUN" "Windows only: scripts\\ci-local.ps1 on the PC"

echo
printf '%s\n' "${results[@]}"
echo
if $ok; then
    echo "LOCAL CI (Linux): OK"
else
    echo "LOCAL CI (Linux): FAILED"
    exit 1
fi
