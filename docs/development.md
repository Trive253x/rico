# Development

## Requirements

- stable Rust ≥ 1.85 ([rustup.rs](https://rustup.rs));
- on Windows the Visual Studio Build Tools (C++), for the linker, with the
  CMake component to build libopus (if `cmake` is not on the PATH, the `CMAKE`
  variable must point to `cmake.exe`);
- for the installer: Inno Setup 6;
- for the local CI: `cargo-audit` and `cargo-deny`, optionally Gitleaks,
  Semgrep and the GitHub CLI (`gh`, needed to publish releases).

The reference development PC is Windows ("Portatile Casa"): that is where the
code is built and where the GUI and the demo are tried. Environments without
access to crates.io cannot build.

## Commands

```sh
cargo fmt                                    # formatting (rustfmt.toml: 110 character lines)
cargo clippy --all-targets -- -D warnings    # no warnings allowed
cargo test                                   # all tests
cargo test -- --include-ignored              # manual tests too (they skip if the input is missing)
cargo build --release                        # optimised binaries
cargo run --release -- --demo                # demo GUI
cargo run --release -- --demo --lang it      # demo GUI in Italian
```

The local CI uses `--locked`: after adding a dependency, `Cargo.lock` must
be committed.

On Windows, close ferrocord before `cargo build --release`: if the executable
is in use the build fails with "Access denied".

## Trying without an account

**Every test is done in demo mode** (`--demo`), not with a real Discord
account. The demo covers: channels and messages, sending, images and the
full-screen view, animated GIFs (links too), GIF and emoji picker, server
emojis, voice chat (local fake voice server: two real DAVE participants
talking in turn with a synthetic voice, end-to-end encrypted; the microphone
stays off until the microphone button is pressed) and both languages.

Tests with a real account are done only when the account owner explicitly
asks, and never sending messages without their permission.

## Tests

The tests always run in English (the default language): none of them changes
the global language, because tests run in parallel.

| Where | What they test |
|---|---|
| `discord/*` | payload parsing, zlib, token, queries |
| `discord/voice/*` | encryption (both modes, RTP extensions and padding, tampered packets), RTP, IP discovery; DAVE (version 0 refused, malformed messages); full call with the fake server and end-to-end encryption: privacy code, Opus audio in and out, microphone off = nothing sent, commit when others join after us, frames without DAVE dropped, call without DAVE refused, second encryption mode, 4017 final, reconnection; voice server addresses |
| `audio/*` | Opus round trip, wrong input, bounded queues, resampling, channel conversion, mixer, voice activity detection |
| `media.rs` | allowed servers, GIF addresses, embeds, decoding of animated GIFs and WebPs, safe file names |
| `text.rs` | Discord tags, custom emojis, `:name:` conversion when sending |
| `store.rs` | history, reactions, unread, voice states |
| `i18n.rs` | language codes and names, month names |
| `gui/*` | interface flows in the demo: navigation, sending, previews, large view, picker, server emojis, voice, channel switching |

Manual test on real files: `FERROCORD_ANIM="a.gif;b.webp" cargo test --lib
animation_from_file -- --ignored --nocapture` decodes the given animations.

## Log

`FERROCORD_LOG=warn,ferrocord=debug` shows downloads, previews, connections
and voice. The file is in `%LOCALAPPDATA%\ferrocord\ferrocord\data\ferrocord.log`
on Windows. Tokens, message text or sent links never go in the log.

## Branches and merging

- `main` is always stable: releases (`v<version>`) are published from it with
  the local CI (below).
- Each feature has its own branch (`feat/…`); it is merged into `main` with a
  pull request and a merge commit (`Merge: … (#n)`).
- No `push --force` or history rewriting on branches already published:
  mistakes are fixed with a new commit.
- Commit messages in English: a short title, then a list of what changes and
  why.

| Branch | State |
|---|---|
| `main` | GUI, images, GIFs, emojis, voice chat with DAVE (PRs #1, #7 and #9) |
| `feat/english-i18n` | everything in English, interface in English or Italian |

## CI (local)

Since 2026-11-08 the "CI" and "Installer Windows" GitHub Actions workflows
are disabled (owner's choice): their work runs on the Windows development PC.

```powershell
powershell -ExecutionPolicy Bypass -File scripts\ci-local.ps1                                # before merging into main
powershell -ExecutionPolicy Bypass -File scripts\ci-local.ps1 -Publish                       # fix release (0.x.PATCH)
powershell -ExecutionPolicy Bypass -File scripts\ci-local.ps1 -Publish -Candidate -Confirmed # feature release candidate (v0.X.0-rc.N)
powershell -ExecutionPolicy Bypass -File scripts\ci-local.ps1 -Publish -Confirmed            # feature release (v0.X.0)
```

Feature releases (`0.X.0`, and every major from 1.0) go out as candidates
first; each candidate and the final release are published only after the
owner confirmed them (`-Confirmed`, see AGENTS.md rule 10).

| Step | What |
|---|---|
| Quality | `cargo fmt --check`, `clippy --all-targets --locked -D warnings`, `cargo test --locked`, release build |
| Security | the "Sicurezza" workflow's tools: `cargo audit`, `cargo deny` (bans, licenses, sources), Semgrep (`p/rust`, `p/secrets`, `p/github-actions`), Gitleaks (whole history); judged by `.github/scripts/security_report.py`, so the same findings fail locally and on GitHub. Report in `target\ci-local\security-report.md` |
| Installer | Inno Setup (`installer/ferrocord.iss`) with the version from `Cargo.toml`: `installer\Output\ferrocord-setup-<version>.exe` |
| Release (`-Publish`) | only from a clean `main` equal to `origin/main`: tag `v<version>` (or `v<version>-rc.N`, pre-release), GitHub release with the installer, its SHA-256 and the CHANGELOG section |

Logs of every step are in `target\ci-local\`; the summary says what passed,
what failed and what was not run (a step not run is not a step passed). Not
covered locally: builds and tests on Linux and macOS, demo screenshots.

The workflow files stay in `.github/workflows/` and can be re-enabled from the
repository's Actions page. The "Sicurezza" workflow (weekly and on every
push) is still enabled on GitHub; Dependabot still proposes updates. The old
`nightly` release stopped at 0.1.0.33.

Tools on the PC: Rust, Visual Studio Build Tools with CMake, Inno Setup 6,
`cargo-audit`, `cargo-deny`, GitHub CLI and Gitleaks (winget), Python 3.12
with Semgrep (`pip install --user semgrep`). Publishing needs `gh auth login`
once.

## Code conventions

- Comments, test names, logs and documentation in English. Every text the
  user reads goes through `i18n::t` / `tf!` with both the English and the
  Italian version.
- Data models with `#[serde(default)]` on optional fields: a missing field
  must not make a whole event fail.
- The network code does not depend on the interface: events on a channel,
  commands through a handle.
- Every download goes through `media::fetch`, which allows only the servers on
  the list and limits the size.
- Widgets that appear and disappear sit in layers of the root `stack`, so
  that the others do not lose their state (see
  [architecture.md](architecture.md)).
