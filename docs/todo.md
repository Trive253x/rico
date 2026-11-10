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

| # | What | Why | Size |
|---|---|---|---|
| 1.2 | The server column is rebuilt (`store.rail()`, folder maps) at every redraw: keep it in the store, updated when servers or folders change. | AGENTS.md "Speed": no work in `view`. Cheap today, grows with the servers. | S |
| 1.3 | Log the host of the voice server (not the token), so the `*.discord.media` limit can be checked afterwards. | Technical event, allowed in the log; makes a refused address diagnosable. | S |

## 2. Checks on real Discord

| # | What | Who |
|---|---|---|
| 2.1 | Per-person volume and local mute heard in a real call (saving and restoring already checked on the owner's account). | test in Episcopali, microphone off |
| 2.2 | Moderation actions (server mute/deafen, move, disconnect, timeout, kick, ban). Irreversible: only on a server of the owner, with a person who agrees. | the owner |
| 2.3 | Stream window on a real stream: people column, full screen, small preview on hover. | test with a real stream |

## 3. Features, compared with the official client

| # | What | Size |
|---|---|---|
| 3.1 | Reply to a message, add and remove reactions, upload files from the GUI. | M |
| 3.2 | Windows notifications for direct messages and mentions, mention badges on servers and channels. | M |
| 3.3 | Edit and delete your own messages. | S |
| 3.4 | Previews of normal links and bot "card" messages (embeds), spoilers, animated avatars. | M |
| 3.5 | Your status (online, idle, do not disturb, invisible) and custom status. | S |
| 3.6 | Friends list and friend requests, starting a new direct message, joining a server with an invite. | M |
| 3.7 | Voice: push-to-talk, noise suppression and echo cancellation, verification code per person (today one code for the whole call). | M |
| 3.8 | Threads and forums, pinned messages, search in messages. | L |
| 3.9 | Stickers and bot commands (`/command`). | M |
| 3.10 | Your own Go Live stream and the webcam. | L |
| 3.11 | Server management: roles, channels, invites, server settings (today only moderation of members). | L |
| 3.12 | Terminal interface (TUI) at the level of the GUI: member list, settings, folders, streams. | L |

## 4. Releases and tools

| # | What | Who |
|---|---|---|
| 4.1 | Signed installer, so SmartScreen stops warning (options: Certum Open Source with a public repository, Certum OV, Azure Artifact Signing only for US/Canada). | owner's choice |
| 4.2 | Builds and tests for Linux and macOS (today only Windows is built locally). | M |
| 4.3 | Delete the old `nightly` release and the old pre-releases on GitHub. | the owner (refused to the assistant) |
