#!/usr/bin/env bash
# Windows installer and GitHub release for Rico, built on Linux: steps 3 and
# 4 of scripts/ci-local.ps1, with the same rules (AGENTS.md, rule 10).
#
#   scripts/release-linux.sh                                  checks + installer
#   scripts/release-linux.sh --publish                        patch release (0.x.PATCH) from main
#   scripts/release-linux.sh --publish --candidate --confirmed  candidate (v0.x.y-rc.N)
#   scripts/release-linux.sh --publish --confirmed            feature release (v0.X.0)
#   --skip-ci   skip scripts/ci-linux.sh (only when it just passed on this commit)
#
# Steps:
#   1. scripts/ci-linux.sh (quality, security, Windows clippy);
#   2. rico.exe and rico-tui.exe for x86_64-pc-windows-gnu, cross-compiled with
#      MinGW-w64, the C++ runtime linked statically (no extra DLL to ship);
#   3. the installer: installer/rico.iss, unchanged, with Inno Setup 6 run
#      under Wine (installed on first use in target/ci-local/wine, from a
#      pinned download checked with its SHA-256);
#   4. --publish: only from a clean `main` equal to origin/main, with the
#      GitHub CLI signed in: tag and GitHub release with the installer, its
#      SHA-256 and the CHANGELOG section. Feature versions (x.y.0) and every
#      candidate only with --confirmed, passed only after the owner confirmed
#      that specific publication.
#
# Differences from ci-local.ps1: the compiler is MinGW, not Visual Studio,
# and the tests are not run on Windows. The last line is
# "LOCAL RELEASE (Linux): OK" or "LOCAL RELEASE (Linux): FAILED".

set -u
cd "$(dirname "$0")/.." || exit 2

publish=false candidate=false confirmed=false skip_ci=false
for arg in "$@"; do
    case "$arg" in
        --publish) publish=true ;;
        --candidate) candidate=true ;;
        --confirmed) confirmed=true ;;
        --skip-ci) skip_ci=true ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

logs=target/ci-local
mkdir -p "$logs"
target=x86_64-pc-windows-gnu
version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
branch=$(git rev-parse --abbrev-ref HEAD)
commit=$(git rev-parse HEAD)
results=()
ok=true
add() { results+=("$(printf '%-10s %-8s %s' "$1" "$2" "${3:-}")"); }
fail() { add "$1" "$2" "${3:-}"; ok=false; }

# A candidate is built as <version>-rc.N: the program (RICO_PRERELEASE) and
# the installer know it is older than its final release (in-app updates).
full=$version
unset RICO_PRERELEASE
if $publish && $candidate; then
    n=1
    while [ -n "$(git ls-remote --tags origin "refs/tags/v$version-rc.$n")" ]; do n=$((n + 1)); done
    full="$version-rc.$n"
    export RICO_PRERELEASE="rc.$n"
    echo "Candidate build: $full"
fi

# Inno Setup 6.7.4, as published by its author (jrsoftware/issrc on GitHub).
INNO_URL=https://github.com/jrsoftware/issrc/releases/download/is-6_7_4/innosetup-6.7.4.exe
INNO_SHA256=a549929319f9120716677c685535ca4671e4b937b5fdc87e2585f32d8eb30e8c

# 1. Checks
if $skip_ci; then
    add ci "NOT RUN" "--skip-ci"
elif scripts/ci-linux.sh >"$logs/ci-linux.log" 2>&1; then
    add ci ok "scripts/ci-linux.sh"
else
    fail ci FAILED "see $logs/ci-linux.log"
fi

# 2. Windows programs
exe=target/$target/release/rico.exe
if $ok; then
    echo "==> windows build"
    static="$logs/static-libs"
    mkdir -p "$static"
    # Only the static C++ runtime in a folder searched first: the linker
    # cannot pick libstdc++-6.dll.
    stdcpp=$(x86_64-w64-mingw32-g++ -print-file-name=libstdc++.a 2>/dev/null)
    pthread=$(x86_64-w64-mingw32-gcc -print-file-name=libwinpthread.a 2>/dev/null)
    if [ ! -f "$stdcpp" ] || [ ! -f "$pthread" ]; then
        fail build "NOT RUN" "MinGW-w64 (posix) not installed"
    else
        cp "$stdcpp" "$static/libstdc++.a"
        cp "$pthread" "$static/libpthread.a"
        if CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
            CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
            CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++ \
            AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar \
            CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS="-L native=$PWD/$static -C link-arg=-static -C link-arg=-static-libgcc -C link-arg=-static-libstdc++" \
            cargo build --release --locked --target "$target" --bin rico --bin rico-tui >"$logs/windows-build.log" 2>&1; then
            # No MinGW DLL may be needed next to the programs.
            extra=$(for f in target/$target/release/rico.exe target/$target/release/rico-tui.exe; do
                x86_64-w64-mingw32-objdump -p "$f" | grep "DLL Name" | grep -Ei "libstdc|libgcc|winpthread"
            done)
            if [ -n "$extra" ]; then
                fail build FAILED "needs MinGW DLLs: $extra"
            else
                add build ok "rico.exe, rico-tui.exe (MinGW, static C++ runtime)"
            fi
        else
            fail build FAILED "see $logs/windows-build.log"
        fi
    fi
else
    add build "NOT RUN" "earlier steps failed"
fi

# 3. Installer
setup=installer/Output/rico-setup-$full.exe
hash=
if $ok; then
    echo "==> installer"
    export WINEPREFIX="${RICO_WINEPREFIX:-$PWD/$logs/wine}" WINEDEBUG=-all
    iscc="$WINEPREFIX/drive_c/Inno/ISCC.exe"
    run_x() { if command -v xvfb-run >/dev/null 2>&1; then xvfb-run -a "$@"; else "$@"; fi; }
    if ! command -v wine >/dev/null 2>&1; then
        fail installer "NOT RUN" "Wine not installed (wine64 and wine32:i386)"
    else
        if [ ! -f "$iscc" ]; then
            tmp=$(mktemp -d)
            if curl -sSfL --retry 3 -o "$tmp/is.exe" "$INNO_URL" &&
                echo "$INNO_SHA256  $tmp/is.exe" | sha256sum -c --quiet -; then
                run_x wine "$tmp/is.exe" /VERYSILENT /SUPPRESSMSGBOXES /CURRENTUSER /NORESTART '/DIR=C:\Inno' \
                    >"$logs/inno-install.log" 2>&1
            fi
            rm -rf "$tmp"
        fi
        if [ ! -f "$iscc" ]; then
            fail installer "NOT RUN" "Inno Setup could not be installed under Wine (see $logs/inno-install.log)"
        else
            # installer/rico.iss takes the programs from target\release.
            cp "target/$target/release/rico.exe" "target/$target/release/rico-tui.exe" target/release/
            rm -f "$setup"
            if wine "$iscc" /Q "/DAppVersion=$full" "$(winepath -w installer/rico.iss)" >"$logs/installer.log" 2>&1 &&
                [ -f "$setup" ]; then
                hash=$(sha256sum "$setup" | cut -d' ' -f1)
                add installer ok "$setup, SHA-256 $hash"
            else
                fail installer FAILED "see $logs/installer.log"
            fi
            rm -f target/release/rico.exe target/release/rico-tui.exe
        fi
    fi
else
    add installer "NOT RUN" "earlier steps failed"
fi

# 4. Release
if $publish; then
    tag="v$version"
    why=
    feature=false
    [[ "$version" =~ ^[0-9]+\.[0-9]+\.0$ ]] && feature=true
    if ! $ok; then
        why="earlier steps failed"
    elif $candidate && ! $confirmed; then
        why="a candidate is published only after the owner asked for it (--confirmed)"
    elif $feature && ! $confirmed; then
        why="$version is a feature release: publish only after the owner confirmed it (--confirmed)"
    elif [ "$branch" != main ]; then
        why="on branch $branch, releases come only from main"
    elif [ -n "$(git status --porcelain --untracked-files=no)" ]; then
        why="uncommitted changes"
    elif ! command -v gh >/dev/null 2>&1; then
        why="GitHub CLI (gh) not installed"
    elif ! gh auth status >/dev/null 2>&1; then
        why="GitHub CLI not signed in: run 'gh auth login'"
    else
        git fetch -q --tags origin main
        if [ "$(git rev-parse origin/main)" != "$commit" ]; then
            why="main differs from origin/main: push or pull first"
        elif [ -n "$(git ls-remote --tags origin "refs/tags/$tag")" ]; then
            why="tag $tag already exists: bump the version"
        fi
    fi
    # The candidate number chosen before the build.
    $candidate && tag="v$full"
    # In-app updates: the signed manifest, when this build has the owner's
    # public keys (rico-sign status: 0 = yes, 3 = no keys yet). The everyday
    # key comes from the system keychain (rico-sign keygen).
    assets=("$setup")
    if [ -z "$why" ]; then
        signer=target/release/rico-sign
        manifest="$logs/rico-update.json"
        rm -f "$manifest" "$manifest.sig"
        if ! cargo build --release --locked --bin rico-sign >"$logs/rico-sign-build.log" 2>&1; then
            why="rico-sign could not be built (see $logs/rico-sign-build.log)"
        else
            keys=0
            "$signer" status >/dev/null 2>&1 || keys=$?
            channel=stable
            $candidate && channel=candidate
            if [ "$keys" = 0 ]; then
                if "$signer" manifest "$setup" --version "$full" --channel "$channel" \
                    --notes "https://github.com/Trive253x/rico/releases/tag/$tag" --out "$manifest" &&
                    "$signer" sign "$manifest" && "$signer" verify "$manifest"; then
                    add manifest ok "rico-update.json signed ($channel)"
                    assets+=("$manifest" "$manifest.sig")
                else
                    why="the update manifest could not be signed"
                fi
            elif [ "$keys" = 3 ]; then
                add manifest "NOT RUN" "no public keys in this build yet: no in-app update for this release"
            else
                why="rico-sign status failed"
            fi
        fi
    fi
    if [ -n "$why" ]; then
        fail release "NOT RUN" "$why"
    else
        notes="$logs/release-notes-$tag.md"
        kind=
        $candidate && kind="Release candidate, for testing before the final release. "
        {
            echo "${kind}Windows installer built on Linux (scripts/release-linux.sh: MinGW-w64 and Inno Setup under Wine) from \`main\` (${commit:0:7}). Not signed: Windows SmartScreen may ask for confirmation on the first start."
            echo
            echo "SHA-256 of \`$(basename "$setup")\`:"
            echo '```'
            echo "$hash"
            echo '```'
            echo
            echo "Video from Go Live streams is decoded with OpenH264 built from source: Cisco's H.264 patent licence does not cover it (see README)."
            echo
            echo "### Changes in $version"
            # This version's CHANGELOG section, up to the next one.
            awk -v v="## $version" 'index($0, v) == 1 { on = 1; next } on && /^## / { exit } on { print }' CHANGELOG.md
        } >"$notes"
        flag=--latest
        $candidate && flag=--prerelease
        if git tag -a "$tag" -m "Rico ${tag#v}" "$commit" && git push -q origin "$tag" &&
            gh release create "$tag" "${assets[@]}" --verify-tag --title "Rico ${tag#v}" --notes-file "$notes" "$flag" \
                >"$logs/release.log" 2>&1; then
            add release ok "$tag"
        else
            fail release FAILED "see $logs/release.log"
        fi
    fi
fi

echo
printf '%s\n' "${results[@]}"
echo
echo "Not covered here: tests on Windows, the Visual Studio build of scripts\\ci-local.ps1."
if $ok; then
    echo "LOCAL RELEASE (Linux): OK"
else
    echo "LOCAL RELEASE (Linux): FAILED"
    exit 1
fi
