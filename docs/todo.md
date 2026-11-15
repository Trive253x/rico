# Things to implement

The list of what ferrocord still lacks, compared with the official Discord
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

## 1. Rules and security

Nothing open at the moment.

## 2. Checks on real Discord

Nothing open at the moment: per-person volume, moderation, the stream
window, replies, reactions, files, editing and deleting, going to the
replied message and the email login were tried by the owner on Discord
(2026-11-15).

## 3. Features, compared with the official client

| # | What | Size |
|---|---|---|
| 3.1 | A button to choose files to upload (today only drag and drop): needs a file dialog, so a new dependency to approve (e.g. `rfd`). | S |
| 3.2 | Windows notifications for direct messages and mentions, mention badges on servers and channels. | M |
| 3.4 | Previews of normal links and bot "card" messages (embeds), spoilers, animated avatars. | M |
| 3.5 | Your status (online, idle, do not disturb, invisible) and custom status. | S |
| 3.6 | Friends list and friend requests, starting a new direct message, joining a server with an invite. | M |
| 3.7 | Voice: push-to-talk, noise suppression and echo cancellation, verification code per person (today one code for the whole call). | M |
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
