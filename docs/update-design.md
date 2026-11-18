# In-app updates: design

Status: **proposal**, not implemented. Written 2026-11-18 for the owner to
decide; nothing here changes how Rico behaves today.

Goal: Rico updates itself from its GitHub releases, without downloading the
installer by hand, as safely and as quickly as possible, with an on-screen
notice while it happens.

## What an attacker could try

An updater downloads a program and runs it: it is the most dangerous code in
Rico. The threats it must resist:

| Threat | Protection |
|---|---|
| Someone takes over the GitHub account or the repository and publishes a malicious installer (with a matching SHA-256 in the notes) | **Signed manifest**: Rico accepts only updates signed with the owner's private key, which never touches GitHub. A SHA-256 published next to the file proves nothing, because whoever can change the file can change the hash |
| Network attacker (fake Wi-Fi, proxy) | HTTPS with certificate and host name checks (as everywhere in Rico), only GitHub's hosts, and the signature anyway |
| Downgrade to an old, vulnerable version (an old, validly signed manifest served again) | The signed manifest carries the version; Rico refuses anything not newer than itself |
| Freeze (an old manifest kept being served, so no update is ever seen) | The manifest carries its signing date; one older than 90 days is reported as "cannot check for updates" |
| Swapping the file between the check and the start of the installer | The installer is written in Rico's own data folder, checked, and kept open without sharing write access until the installer process has started |
| A huge or endless download | Size limit from the signed manifest (and a hard cap of 64 MB), timeouts, one download at a time |
| Theft of the signing key | The key lives only on the owner's PC, encrypted with a passphrase; Rico knows **two** public keys (everyday and spare, the spare kept offline) so a stolen key can be replaced by a release signed with the spare |
| Running an update the user did not want | Nothing is installed without a click (or the option the user turns on); candidates only for who asks for them |

Not covered: Windows' own trust in the installer. Without a code-signing
certificate (todo 4.1) SmartScreen may still warn when the installer is
started by hand; an update started by Rico is not downloaded by a browser, so
the "downloaded from the Internet" mark is not added to it.

## The pieces

### 1. The manifest, published with each release

Next to `rico-setup-<version>.exe`, each release gets:

- `rico-update.json`:
  ```json
  {
    "version": "0.17.0",
    "channel": "stable",
    "file": "rico-setup-0.17.0.exe",
    "size": 12111270,
    "sha256": "…",
    "signed_at": "2026-11-18T12:00:00Z",
    "notes_url": "https://github.com/Trive253x/rico/releases/tag/v0.17.0"
  }
  ```
  (`"channel": "candidate"` for `v0.x.y-rc.N`.)
- `rico-update.json.sig`: an Ed25519 signature of the exact bytes of the JSON.

Ed25519: small keys and signatures, fast, no parameters to get wrong. The
verification uses `ed25519-dalek` (strict verification), already in
`Cargo.lock` through DAVE's OpenMLS: no new crate.

Signing happens only where the private key is, the owner's PC:
`scripts\ci-local.ps1 -Publish` (and `scripts/release-linux.sh --publish`
on a Linux PC) sign the manifest with a small `rico-sign` tool built from the
repository (same crate), reading the encrypted key from a file outside the
repository and the passphrase from the keyboard. Cloud sessions never see the
key, so they cannot publish updates (they cannot publish releases today
either).

### 2. Checking (fast, light, private)

- **One small request**: `https://github.com/Trive253x/rico/releases/latest/download/rico-update.json`
  (and its `.sig`). No GitHub API, no token, no rate limit worries; a few
  hundred bytes. Candidates: the newest release's manifest through the
  releases list, only for who chose candidates.
- **When**: 30 seconds after the window is ready (never during the start, so
  the < 1 s start target is untouched), then every 24 hours while Rico runs;
  never on the interface thread (a `Task`), never in the demo.
- `If-None-Match` with the last `ETag`: an unchanged manifest costs one
  empty answer.
- **Privacy**: GitHub sees the IP address at each check, once a day at most.
  So it is **off until the user chooses**: at the first start after this
  version, a one-time question "Check for updates on GitHub? (GitHub sees your
  IP address once a day)" with Yes / No; changeable in Settings › About.

### 3. Downloading (in the background)

- A dedicated `media::fetch_update`: HTTPS only, at most 3 redirects, **each
  hop's host checked** against `github.com`, `objects.githubusercontent.com`
  and `release-assets.githubusercontent.com` (the release files are served
  through a redirect to the latter), no cookies, no credentials, timeouts,
  at most the manifest's `size` and never more than 64 MB.
- It starts as soon as an update is found (the user keeps chatting), into
  `%LOCALAPPDATA%\rico\rico\data\updates\rico-setup-<version>.exe`, written to
  a `.part` file and renamed only after the SHA-256 matches the signed
  manifest. Old files in that folder are deleted (only ours, by name).
- Paused when the network is metered? Windows tells it; first version: no,
  12 MB once.

### 4. Installing (quick, nothing to answer)

- Rico shows "Rico 0.17.0 is ready — Restart to update" (and "What's new").
  Two choices: **Restart now**, or **Update when I close Rico** (then the
  next close installs it: zero waiting).
- Before starting the installer Rico checks the file again (size, SHA-256
  against the signed manifest), keeps it open without write sharing, starts
  it with
  `/SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /UPDATE=1`
  and then closes itself (voice calls and streams are left first, so the
  microphone and screen capture are released).
- The installer keeps the user's choices (folder, shortcuts, "for me" or
  "for everyone"); a per-user installation needs no administrator rights,
  so no question at all. An installation "for everyone" shows Windows'
  administrator prompt: that is expected and stays.
- **Change needed in `installer/rico.iss`** (owner's approval: it is on the
  do-not-touch list): a second `[Run]` line that starts Rico again after a
  silent update, only when `/UPDATE=1` is given:
  ```
  Filename: "{app}\rico.exe"; Parameters: "--updated"; Flags: nowait; Check: IsUpdate
  ```
  with a three-line `IsUpdate` function reading `{param:UPDATE|0}`.

### 5. The on-screen notice

- **Before**: a small bar at the top of the window: "Downloading Rico
  0.17.0… 42%", then "Ready — Restart to update" with the two buttons. It
  never covers the chat and goes away with ×.
- **During**: `/SILENT` makes Inno Setup show its small progress window
  ("Installing Rico…", with Rico's icon and a progress bar), without any
  question; it lasts a few seconds.
- **After**: Rico starts again by itself (`--updated`) and shows for a few
  seconds "Rico updated to 0.17.0 — What's new", which opens the release
  notes.
- Later, with Windows notifications (todo 3.2), the same three moments can
  also be system notifications.

## Speed

| Step | Cost |
|---|---|
| Start of Rico | none (the check waits 30 s) |
| Check | 2 small HTTPS requests a day, off the interface thread |
| Download | about 12 MB in the background |
| Signature + SHA-256 | a few milliseconds, off the interface thread |
| Install | a few seconds with the progress window, then Rico starts again |

Delta updates (only the changed bytes) would save a few MB but add a lot of
code to verify; not worth it at 12 MB.

## Work, in order

1. The signing key (owner, once): `rico-sign keygen` on the PC, the public
   keys into Rico, the encrypted private key and the spare key kept by the
   owner.
2. `rico-sign` and the release scripts publishing `rico-update.json` and
   `.sig` (and refusing to publish without them).
3. `updater` module: manifest parsing (tolerant, but every field required
   for an update), signature, version and date checks, `fetch_update`,
   file handling; tests for every refusal (bad signature, other key, older
   version, too big, wrong hash, foreign redirect host, expired manifest).
4. Interface: the first-start question, the setting, the bar, the restart,
   the "updated" notice.
5. The `installer/rico.iss` line (with the owner's approval).
6. One release signed the new way; only the one after it can be the first
   real in-app update.

Size: M (a few days), most of it in tests.

## Decisions for the owner

1. Check for updates: asked at the first start (recommended), or on by
   default?
2. Candidates: only stable releases by default, candidates as an option
   (recommended)?
3. Installing: "Restart now" and "Update when I close Rico" (recommended), or
   fully automatic at close?
4. The `installer/rico.iss` change for the restart after the update.
5. The signing key on the PC, with a spare kept offline.
