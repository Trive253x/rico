# Things to implement

The list of what Rico still lacks, compared with the official Discord
client and with the project rules (AGENTS.md). It is the reference for
suggesting the next changes: whoever works on the code (people or AI
assistants) proposes from here, in this order, and keeps it up to date.

How it is kept:

- the order is the order of work: security and rules first, then checks on
  real Discord, then features by how much they are felt in daily use;
- when a pull request completes an item, the item is removed here (it is
  described in CHANGELOG.md) and the README table is updated;
- new gaps found while working are added in the right section, with why
  they matter;
- the owner decides the order: an item moves only when asked.

Sizes are rough: **S** a few hours, **M** a day or two, **L** more.

## Goals for 1.0

Proposed on 2026-11-25; the owner decides when Rico is 1.0 (no date). 1.0
is published as any release, after the owner's confirmation, when all of
these hold:

- **Signed installer** (4.1), so Windows stops warning whoever installs it.
- **Stable voice**: at least one or two weeks of use with 0.20.0 or later
  with no reports of gaps or lag in the voice (DPDFNet, RTP timestamps, the
  1 ms timer).
- **Embeds and spoilers**: done in 0.20.0 (animated avatars, the rest of 3.4, can wait).
- **Threads, pinned messages and search** (3.8; forums can wait).

The rest (3.6, 3.9–3.12, 4.2, 4.4) can come in 1.x.

## 1. Rules and security

Nothing open at the moment.

## 2. Checks on real Discord

- **Voice timer on Windows**: fixed in 0.20.0 (1 ms system timer during
  calls); to check in real calls that the late ticks in the log go down.
- **DPDFNet when the computer is busy**: since 0.20.0 only the 16 kHz model,
  and RNNoise takes over when the clean-up stays too slow (with a notice);
  to check on the owner's PC under load that it switches, and that the
  voice has no gaps afterwards.

Otherwise nothing open: per-person volume, moderation, the stream
window, replies, reactions, files, editing and deleting, going to the
replied message and the email login were tried by the owner on Discord
(2026-11-15).

## 3. Features, compared with the official client

| # | What | Size |
|---|---|---|
| 3.4 | Animated avatars; opening a link in the browser (with a warning, as Discord: a new way out of the app, the owner decides); bots' markdown in cards (`**bold**`, `[text](url)` are shown as written, like in messages); cards and spoilers in the terminal interface (spoilers are hidden there as `▒`, cards not shown). Link previews, bot cards and spoilers are done (0.20.0). | S–M |
| 3.6 | Joining a server with an invite; blocking people and nicknames for friends (the friends list and requests are done in 0.18.0). | S |
| 3.8 | Threads and forums, pinned messages, search in messages. | L |
| 3.9 | Stickers and bot commands (`/command`). | M |
| 3.10 | The webcam (your own screen stream is done). | L |
| 3.11 | Server management: roles, channels, invites, server settings (today only moderation of members). | L |
| 3.12 | Terminal interface (TUI) at the level of the GUI: member list, settings, folders, streams. | L |

## 4. Releases and tools

| # | What | Who |
|---|---|---|
| 4.1 | Signed installer, so SmartScreen stops warning (options: Certum Open Source with a public repository, Certum OV, Azure Artifact Signing only for US/Canada). | owner's choice |
| 4.2 | Builds and tests for Linux and macOS (today only Windows is built locally). | M |
| 4.3 | Delete the old `nightly` release and the old pre-releases on GitHub. | the owner (refused to the assistant) |
| 4.4 | Replace `chrono` (unmaintained, RUSTSEC-2026-0543; no known vulnerability) with `jiff` (or `time`, already in `Cargo.lock`): about 40 uses in 13 files (message times, profile dates, timeout and status expiry, `rico-sign`, update freshness). With tests on times before and after; when a vulnerability appears or at the next dependency round. The other unmaintained crates `cargo audit` reports (`zbus_names`, `zvariant`, `zvariant_derive`, `rustybuzz`, `ttf-parser`, `paste`, `instant`, `pin-utils`, `proc-macro-error2`, `libcrux-aesgcm`) and the unsound `lru` 0.16 come through iced 0.14 and OpenMLS 0.8 (davey): they go away with their updates. Checked 2026-11-23. | M |
