# Changelog

Every implementation changes the version (see AGENTS.md, rule 7): a new
feature bumps the minor number (0.2.0 → 0.3.0), a fix or improvement the
patch number (0.2.0 → 0.2.1). Numbers in brackets are pull requests.

## 0.18.0 — unreleased

- **In-app updates (Windows).** Once a day Rico
  asks GitHub for the newest release's manifest, accepts it only if it is
  signed with the author's Ed25519 key, downloads the installer in the
  background (bar at the top: "Downloading Rico X… 42%", then "Rico X is
  ready" with **Restart now**), checks it again, starts it in silent mode
  (Inno Setup's small progress window) and Rico opens again by itself
  showing "Rico updated to X". Switch in Settings › About, on by default
  (GitHub sees the IP address once a day).
- Against downgrades: only a signed version strictly newer than the running
  one, never below the highest version already seen, the installer bound to
  the signed size and SHA-256, a manifest older than 90 days (or older than
  the last one seen) refused, and the installer itself refusing to go back
  in update mode. A signed list can mark versions with a known flaw: Rico
  then shows a red bar asking to update.
- Downloads only from `github.com` and GitHub's file hosts, every redirect
  checked, at most the signed size (never more than 64 MB), into Rico's own
  data folder; the installer is held open without write sharing between
  the last check and its start.
- The signing keys exist (everyday and offline spare) and their public
  keys are in the program, so updates are active from this version; the
  first in-app update is from 0.18.0-rc.1 to the next signed release.
- `rico-sign`: the tool that makes the keys (everyday one in the system
  keychain, spare one in a file to keep offline), the manifest and its
  signature, on the author's PC only.
- The installer: in update mode it opens Rico again at the end and refuses
  to install a version that is not newer than the installed one, candidates
  included (`0.18.0-rc.1` < `rc.2` < `0.18.0`).
- When both the stable and the candidate check fail, the log names both
  errors (before, a failing candidate check was hidden behind the stable
  one).
- **What's new** button on the "ready" and "updated" bars: opens the
  release notes on GitHub (only Rico's release pages).
- **Release candidates** as an option (Settings › About, off by default):
  the newest candidate is offered when it is newer than the newest stable
  release; the highest version seen is kept per channel, so turning the
  option off never blocks the next stable release.
- Release scripts: candidates are built as `<version>-rc.N` (the program and
  the installer know it), and once the signing key exists each release also
  gets the signed `rico-update.json` and `.sig` (`rico-sign status` tells
  whether the build has keys).

## 0.17.0 — unreleased

- **Right click on a person** — their name or avatar in the messages, the
  member list (every style) and the voice channels — opens their menu at the
  cursor, like on Discord: Profile, Mention (where you can write), Message
  (opens your direct messages with them, or starts them), Mute or Unmute
  only for you, then the moderation your permissions and roles allow: server
  mute / unmute, deafen / undeafen and disconnect while they are in a voice
  channel of that server (done at once), Timeout, Kick and Ban: these open a
  small window where the menu was, with the question and Confirm / Cancel
  (for Timeout first the length), not the whole user card. The outcome goes
  to the status bar ("Marco: kicked from the server."). The permissions and
  the role hierarchy are checked again before anything is sent. Last, Copy
  username and Copy user ID. A click elsewhere, a second right click or Esc
  closes the menu or the small window; only one is open at a time.
- Timeout lengths read "1 day" and "1 week" instead of "1 days" and
  "1 weeks".
- "Message" asks Discord for the conversation only when there is none yet
  (`POST /users/@me/channels`); nothing is sent in the demo.
- Watching a stream: the colours and the coloured edges are now as in the
  official client. The decoder library converted pictures with the colours
  of standard-definition video (BT.601) and repeated each colour sample over
  2×2 pixels; Rico now converts them itself, with BT.709 for HD streams (720
  lines and more) and the colour smoothed between pixels.
- Watching a stream: the video no longer goes black for a moment from time
  to time. Since iced 0.14 a new image is uploaded to the graphics card in
  the background and nothing is drawn until it is ready; with a new picture
  every frame, now and then the frame was empty. Each picture is now
  uploaded before it is shown (about a millisecond) and the previous one
  stays on screen until then. The video also no longer starts black when
  that part of the window is rebuilt.
- QR login: when Discord asks for a captcha, the message now says what
  works: Discord is limiting the QR login for that account, so use the Email
  tab, try again in a few hours, or the token. (A page to solve the captcha
  in the browser was tried in 0.17.0-rc.1 and removed: Discord refuses an
  answer not given on discord.com.)

## 0.16.0 — unreleased

- **Right click on a message** opens its menu where the cursor is: add
  reaction, reply, edit (yours), copy text, copy message link, copy message
  ID and delete (yours, or with the permission to manage messages; asks for
  confirmation as from the bar). Only what the permissions allow is offered
  and checked again when chosen. A click elsewhere or Esc closes it.
- Member list, "by activity" style: the "Offline — N" row no longer wraps on
  two lines in a narrow column (the faces are cut instead).

## 0.15.0 — 2026-11-17

- **Your status**: a click on your name at the bottom left opens a menu to
  be online, idle, do not disturb or invisible, and to set a custom status
  (up to 128 characters) that can go away by itself after 30 minutes, 1
  hour, 4 hours or at midnight. They are saved in the account settings, as
  the official client does, so every device shows them; this session also
  sends them to Discord at once (gateway op 3), after connecting and when
  they change on another device. The dot on your avatar shows the status,
  the line under your name the custom status. "View profile" is in the same
  menu.
- Until now Rico connected as "online" whatever status the account had:
  it now follows the saved one.
- **Attach button** (paperclip) left of the message field: opens the
  system's file dialog; the chosen files go through the same checks as
  dropped ones (permissions, at most 10 files and 10 MB). New dependency:
  `rfd` (Windows' own dialog; on Linux the XDG portal, without GTK).

## 0.14.0 — 2026-11-17

- **Four styles for the member list**, chosen in Settings › Appearance ›
  Member list (saved):
  - **Classic**: as before, groups by role.
  - **Compact**: short rows with small avatars, so about twice as many
    people fit; a search field over the loaded members, the member and
    online counts, a coloured dot for each role group, and "LIVE", "in
    voice", "idle" or "do not disturb" on the right.
  - **Rich**: a bigger row per person with a ring in the status colour and
    a line on what they are doing: streaming or in voice (with the channel),
    typing here, their custom status or game, otherwise idle / do not
    disturb.
  - **By activity**: who is in a voice channel of the server first (LIVE
    badge, muted microphone), then who is typing or wrote in the open
    channel in the last 15 minutes, then online, and offline folded in one
    row with their faces (a click unfolds them).
- Everything shown comes from data Rico already receives (member list,
  voice states, typing, loaded messages): no new requests to Discord.
  Custom statuses and games are read from the member list when Discord
  includes them (control characters removed, at most 128 characters); Rico
  still subscribes with activities off, so they may not appear for
  everyone.
- An unknown member list style in `settings.json` (from a newer version)
  falls back to Classic instead of resetting all the settings.
- Development: `scripts/ci-linux.sh` also runs clippy on the Windows code
  cross-compiled with MinGW, and `scripts/release-linux.sh` builds the
  Windows installer on Linux (MinGW, Inno Setup under Wine) and can publish
  releases with the same rules as `ci-local.ps1 -Publish`. Either local CI
  is now enough before a merge (AGENTS.md, rules 9 and 10).
- **New role card** (a click on `@role`): the role's colour, the faces of
  its first six members (and how many more), how many of them are online
  now and how many Rico has loaded, the online ones as chips to open their
  profile, and "See all" for the whole list.

## 0.13.2 — 2026-11-17

- Mentions can be clicked: `@person` opens that person's profile (with the
  server part in a server), `@role` opens a card with the role's colour and
  the members with that role, sorted by name; a click on a name opens the
  profile. The card lists only the members Rico has loaded so far (member
  list, messages, voice channels) and says so: the server's whole member
  list is never downloaded for it. Esc or a click outside closes it.
- The demo's #general has a message mentioning two roles, to try it.
- Development: `scripts/ci-linux.sh` runs the local CI's quality and
  security checks on Linux (cloud sessions); `ci-local.ps1` on the Windows PC
  is still needed before merging. Fixed what it found: Windows-only screen
  capture code unused on Linux (clippy), a doc comment too long for newer
  clippy, and a screen capture test that failed now and then because it did
  not wait for the previews.

## 0.13.1 — 2026-11-17

- Role mentions (`<@&id>`) show the role's name, as in Discord (`@Moderators`),
  instead of a fixed `@role`: the text was made readable without the
  server's roles. A role that is not in the channel's server (deleted, or of
  another server) shows as `@unknown-role`. Same in the window and in the
  terminal.
- A mention of one of your roles is highlighted like a mention of you, and
  the message gets the mention background, as in Discord.

## 0.13.0 — 2026-11-15

- **ferrocord is now Rico.** The programs are `rico` and `rico-tui`, the
  window and the help say Rico, the installer is `rico-setup-<version>.exe`
  and the repository is `Trive253x/rico`.
- Nothing is lost moving from ferrocord: at the first start the settings,
  language and log folders are moved to the new ones (`…\rico\rico\…`),
  and the token saved in the `ferrocord` keychain entry is moved to the
  `rico` one (the old entry is deleted only once the new one is saved).
  Logging out deletes both entries.
- The installer keeps the same AppId, so it updates the ferrocord
  installation in place, and removes `ferrocord.exe`, `ferrocord-tui.exe`
  and the old Start menu and desktop shortcuts.
- The environment variables are now `RICO_TOKEN`, `RICO_LANG` and
  `RICO_LOG`; the `FERROCORD_…` ones still work.
- Discord sees the client as `rico` (user agent and gateway properties).
- **Seven new themes** in Settings › Appearance: five dark ones (Forest,
  Ocean, Sunset, Graphite, Rosé Pine) and two light ones (Solarized light,
  Mint). A test checks that every theme keeps text, secondary text and
  button labels readable (contrast of at least 4.5:1); the accent of the
  light theme is a touch darker to pass it.

## 0.12.0 — 2026-11-15

- **Window on iced 0.14** (from 0.13), the graphical library of the whole
  interface. The code follows its new API: spaces and rules, widget ids
  and operations (focus, scroll), the program builder, subscriptions (a
  small `keyed` helper keeps each connection running while its key stays
  the same, as before), the GIF and stream video widgets (`update` instead
  of `on_event`), the new style fields (button pixel snapping off, the
  middle-click scroll overlay in the app's colours). Nothing is meant to
  look or behave differently; to be checked by hand.
- With iced 0.14, `image` 0.24 → 0.25 (the same copy iced uses now) and
  `dark-light` 1.1 → 3 (iced no longer uses it, so ours is the only copy).
- ratatui 0.30 leaves control characters out of the terminal cells by
  itself; ferrocord's own filter stays, and has a test of its own.
- Rust 1.88 is now the minimum (iced 0.14, wgpu and image need it).
- README: a comparison with the official client measured on the owner's PC
  (memory, processes, disk, start), the icons of the message bar and the
  jump to the replied message.
- Terminal interface on the current libraries: `ratatui` 0.29 → 0.30 and
  `crossterm` 0.28 → 0.29, updated together (ratatui 0.30 uses crossterm
  0.29, so no second copy). No code change was needed; it is the base for
  bringing the terminal to the level of the window (docs/todo.md, 3.12).
- The crypto group of Dependabot's #39 (aes-gcm, chacha20poly1305, sha2,
  rand, openmls) waits: `davey`, the DAVE library, in its latest version
  (0.2.0) still uses openmls 0.8 and the current crypto crates, so
  updating ours would only add second copies.

## 0.11.1 — 2026-11-15

- Dependencies updated, in the first of the groups that replace Dependabot's
  pull request #39 (which with all 18 updates together did not build):
  `tokio-tungstenite` 0.24 → 0.30 (gateway, voice and QR connections),
  `directories` 5 → 6 (where settings and logs are kept: same folders as
  before), `emojis` 0.6 → 0.9 (emoji picker), and the compatible fixes of
  `flate2`, `openh264` and `unicode-width`. Nothing changes for the user.
- The button that stops your stream says "Stop" (in Italian too) and stays
  on one line in a narrow window: the name of what you stream is cut
  instead, where before the button was squeezed into a vertical column.
- `image`, `dark-light` and `base64` stay where they are: iced 0.13 and
  reqwest still use the current versions, and updating ours would only put
  two copies in the program. They will follow iced 0.14.

## 0.11.0 — 2026-11-14

- **Reply** to a message: "Reply" in the bar that appears over a message
  under the mouse. A line above the text field shows who you are replying to,
  with an `@ ON/OFF` switch to notify them or not (on by default, as in
  Discord; off when replying to yourself), × or Esc to cancel.
- **Reactions**: a click on a reaction adds yours or removes it; "React" opens
  the emoji picker, whose emoji (Unicode or of the channel's server) becomes a
  reaction instead of text. The count changes when Discord confirms it on the
  gateway, so nothing is counted twice.
- **Upload files**: drag files onto the window; they appear above the text
  field (× removes one) and go with the next message, only when you press
  Send. At most 10 files and 10 MB per message (Discord's limit without
  Nitro), checked before reading them; only the file name is sent, never the
  folder it was in. Nothing is uploaded in the demo.
- **Edit and delete your messages**: "Edit" puts the text in the field
  (Enter saves, Esc cancels and gives back what you were writing); "Delete"
  asks for a confirmation in the same bar. Emptying the text of an edit asks
  to delete the message, as in Discord.
- Every action is checked in the shared `Store` (`can_reply`, `can_react`,
  `can_attach`, `can_edit`, `can_delete`) before it is offered and again
  before the request: for example no new reactions without "Add Reactions",
  no files without "Attach Files", no edits where you can no longer write.
- Security fix: a channel ferrocord does not know (neither a server channel
  nor one of your direct messages) now gets no permission, instead of all of
  them as a direct message. It could not be opened from the interface, so
  nothing was reachable this way, but missing data must never allow anything.
- A reply, an edit or files waiting are dropped when you open another
  channel: they never end up somewhere else.
- The actions over a message are small line icons (react, reply, edit, red
  bin for delete) with their name in a tooltip, without borders; drawn for
  ferrocord like the call controls.
- **Go to the replied message**: a click on the line above a reply scrolls to
  the message it answers and highlights it for a moment; if it is not loaded
  yet, older pages are loaded first (at most 10, 500 messages).
- The GIF and emoji buttons next to the text field are line icons too.
- Names written with decorative Unicode characters (mathematical letters,
  small capitals and similar, as many servers use for categories and
  channels) are drawn with the system's fonts as fallback instead of empty
  boxes: category titles, the server name over the chat and the emoji picker.

## 0.10.0 — 2026-11-14

- **Stream your screen** (Go Live) in a server's voice channel: the screen
  button in the call panel opens the choice of a screen or a window, the
  quality (720p 30 fps, what Discord allows without Nitro; 720p 60, 1080p 30
  or 480p 30) and the sound. The sound is the streamed window's program, or
  for a whole screen every program except ferrocord, so the voices of the
  call never end up in the stream. Video is H.264 (OpenH264, the same library
  that decodes streams), end-to-end encrypted with DAVE like the voice;
  viewers' requests for keyframes and lost packets are answered.
- While you stream, "LIVE" with a small preview of what you send and the
  stop button stays in the call panel; Windows' yellow border around the
  captured item is turned off where Windows allows it (Windows 11). Leaving
  the call, closing the streamed window or Discord ending the stream stops
  it, and stopping releases the screen and the sound.
- Two new dependencies, Windows only, approved by the owner:
  `windows-capture` (screen and window capture) and `wasapi` (the sound of a
  program). Neither adds `unsafe` code to ferrocord.
- Demo: the test picture (with a tone) can be streamed to the local test
  server, where a fake viewer decrypts it.
- Cost of a stream of a 1920×1080 screen at 720p 30, measured in the demo:
  copy 1.5 ms, scaling 5.9 ms, encoding 7 ms per picture (scaling was
  10.1 ms in the candidates, see below). Pictures Windows delivers beyond the frame rate are dropped
  before being copied from the graphics card. The build now warns when NASM
  is missing: OpenH264 then has no assembly and encoding takes about 6 times
  longer. The log says the average times every 5 seconds while streaming.
- Tested on Discord: the official client shows the stream. Two fixes found
  there: H.264 goes out with payload type 101 (103 is H.265 for Discord)
  and the playout-delay element is written as Discord's clients do, without
  which the server dropped the video (error 2012 for viewers). While
  streaming, the debug log counts the viewers' reports (numbers only).
- **Login with email and password**, in the GUI and in the terminal, with
  the code of the authenticator app when the account has two-factor
  authentication. The password goes only to Discord and is never saved or
  written to the log: ferrocord keeps only the token, in the system's
  keychain as before. Accounts that confirm logins only by SMS or security
  key are told to use the QR code.
- **New login location**: when Discord wants the login from a new place
  authorized, it sends an email; ferrocord now says so and brings back the
  login form, to sign in again once the email has been approved. The QR
  login says the same instead of "use the token".
- When Discord asks for a captcha (which ferrocord cannot show), the
  message suggests signing in once on discord.com from the same network and
  trying again.
- **Faster screen streaming** than in the candidates: turning the captured
  screen into the picture sent is 42% faster for 720p from a 1080p screen (10.1 → 5.9 ms per
  picture) and almost 5 times faster at the screen's own size (1080p:
  22.7 → 4.8 ms), with exactly the same result (checked by a test against
  the old conversion). For a 1080p stream, conversion plus encoding (about
  18 ms) now fit in the 33 ms of a picture at 30 fps.
- Loading older messages compares them with the ones already shown through
  a set instead of a search per message.
- Code follows more of Rust's community conventions: 35 of clippy's
  pedantic lints are now enforced by `Cargo.toml` (`Self` in impls, let-else,
  `clone_from`, `write!` into strings, lazy defaults, small values passed by
  copy and others); nothing changes for the user.

## 0.9.2 — 2026-11-10

- The server column (servers and folders on the left) is built when servers
  or folders change, instead of at every redraw of the window.
- The log names the voice server's host when a call or stream connects, and
  the refused host when the address is not a `*.discord.media` server (only
  the host, never the token), so a failed connection can be understood.

## 0.9.1 — 2026-11-10

- GIFs of Tenor and Giphy links no longer contact Tenor (Google) and Giphy:
  ferrocord downloads the MP4 clip Discord's own proxy serves for them and
  decodes it on the PC (same decoder as stream video). The sites no longer
  see your IP address, the download is about four times smaller (73 KB
  instead of 292 KB on the measured GIF) and the GIF appears sooner; decoding
  costs about 60 ms of CPU per GIF, in the background. Tenor and Giphy are
  removed from the servers ferrocord may contact.
- GIF picker: its previews come from Klipy (Discord's GIF provider), which
  sees the IP address, so they are now **off until you turn them on** in
  Settings → Text & images. Off, the picker shows the titles (a click still
  sends the GIF) and a line that opens that setting.

## 0.9.0 — 2026-11-10

- Member list on the right of server channels, like Discord's: groups by
  hoisted role and online/offline with their count, avatars with the online
  status, names in their role's colour; "Show more" for the next pages (up
  to 500 rows). The people button in the channel header hides or shows it
  (saved).
- Per-person volume in calls (0–200%) and local mute, from the user card:
  only you hear the difference, it is kept for the next calls (by user ID in
  `settings.json`) and applies at once, also in the middle of a call.
- User card: Mention (writes `@name` in the message being written) and Copy
  ID; the card opens from the member list too, also with the right click.
- Moderation from the user card, only where your permissions and roles
  allow it: server mute and deafen, move to another voice channel,
  disconnect, timeout (1 minute to 1 week, or remove it), kick and ban. Kick,
  ban and timeout ask for a confirmation in the card first.

## 0.8.0 — 2026-11-10

- Stream window like Discord's: the people of the voice channel on the left,
  with who is speaking (green ring), who has the microphone or the sound
  off, and a LIVE badge on whoever else is streaming (a click switches to
  their stream). A button above the video hides or shows them.
- Mouse over someone streaming (in the voice channels or next to the
  stream): a small preview of their stream, like on Discord; a click on
  them watches it (joining the channel if needed), the right click opens
  their profile. The preview is asked only then, at most once a minute per
  stream.
- Full screen for streams: button in the controls, F11 or a double click on
  the video. In full screen only the video shows; the controls appear when
  the mouse goes to the bottom. Esc leaves full screen first, then stops
  watching; closing the stream in any way goes back to the window.

## 0.7.2 — 2026-11-09

Review of the code against the project rules (AGENTS.md):

- Permissions: a server channel whose permissions cannot be computed yet
  (roles or our identity not loaded) now grants nothing and is not listed,
  instead of allowing everything. Private conversations are unchanged.
- Gateway: the resume address Discord gives is used only if it is a
  `wss://…discord.gg` address (the token is sent there); otherwise the
  default one. Compressed gateway messages have a size limit (32 MB
  compressed, 128 MB decompressed) against decompression bombs.
- REST and QR login never follow redirects, and only over HTTPS.
- Voice and streams: the voice server address Discord gives is accepted
  only on `*.discord.media` (the voice token is sent there); before, any
  valid host name was.
- Log: a malformed gateway event is logged with the kind and position of the
  error only, no longer with serde's text, which could quote message text.
- Terminal: control characters (ESC, BEL…) in names, titles and the status
  bar are removed before drawing, not only in message bodies.
- Interface thread: saving the settings and the language, listing the audio
  devices and asking the system theme happen in the background. The "like
  the system" theme no longer checks every 5 seconds: it checks when the
  window gets the focus (no periodic wake-ups at rest).

## 0.7.1 — 2026-11-09

- Voice: other people no longer stutter. The speakers were filled 60 ms
  ahead, as much as the whole cushion of the jitter buffer: a packet even
  10–20 ms late found its turn gone, was replaced by concealment and then
  dropped (about one in eight in a real call). Now the speakers are kept
  40 ms ahead and the cushion is 100 ms: in the same call nothing is
  concealed or late.

## 0.7.0 — 2026-11-09

- Server folders, the same as on Discord: they come from the account
  settings and stay in sync when they change on another device. In the
  server column a closed folder shows its servers' icons on its colour, a
  click opens it.
- Folder editor (button under the servers, or right click on a server or a
  folder): move servers in and out of folders, new folders, name, colour,
  order, ungroup. "Save" sends the folders to the Discord account (only
  them); if they were changed meanwhile on another device nothing is
  overwritten and the editor shows them as they are now.

## 0.6.0 — 2026-11-09

- Settings window (gear next to your name, or Ctrl + ,), grouped like
  Discord's: my account, appearance, accessibility, voice and video, text and
  images, language, keyboard shortcuts, about. Saved in `settings.json`;
  nothing is sent to Discord (account settings will come later).
- Themes: dark, light, black (AMOLED), like the system (follows Windows, also
  when it changes), purple, light brown, blue, red.
- Appearance: interface zoom (80–150%), compact messages (one line, no
  avatars), 24-hour or 12-hour clock.
- GIFs and animated emojis can stay still and play only under the mouse.
- Images and GIFs in the chat can be turned off (nothing is downloaded), and
  so can reactions.
- Voice: choice of microphone and speakers, input and output volume (up to
  200%), automatic or manual microphone sensitivity; changes apply during a
  call too.

## 0.5.0 — 2026-11-09

- Stream view: a control bar under the video with microphone and sound
  buttons, the stream's volume (slider from 0 to 200%, ↑ ↓ keys, a click on
  the speaker mutes it), a clear "Stop watching (Esc)" button and one to
  leave the voice channel. The volume chosen is kept for the next stream;
  turning the sound off silences the stream too.
- Voice panel: round icon buttons for microphone, sound and leave, red when
  muted, with a tooltip that says the state and what a click does.

## 0.4.1 — 2026-11-08

- Voice: no more clicks and jumps when people talk. Received audio goes
  through a jitter buffer per speaker: packets are put back in order, a lost
  packet is rebuilt from the next one (Opus FEC) or concealed instead of
  leaving a hole, playback starts with a 60 ms cushion and a backlog is cut
  back. The speakers are fed when their queue runs low (the sound card's
  clock), no longer by a 20 ms timer that on Windows often fired late.

## 0.4.0 — 2026-11-08

- Watching Go Live streams: "Watch" in the preview card joins the voice
  channel (microphone off) and shows the stream full window, with its sound.
  H.264 video decoded with OpenH264 on its own thread; lost packets are asked
  again (NACK/RTX) and a new keyframe is asked when the picture breaks
  (PLI). Esc stops watching and keeps you in the channel.

## 0.3.0 — 2026-11-08

- Go Live streams: a red LIVE badge next to whoever is streaming in a voice
  channel; a click opens the stream preview (the still image Discord
  updates every few minutes), asked only when opened, with a refresh button.

## 0.2.0 — 2026-11-08

Everything implemented after 0.1.0:

- Images, animated GIFs, emoji and GIF pickers, server emojis (#7).
- Voice chat: Opus audio, microphone and speakers, DAVE end-to-end
  encryption (#9).
- Interface in English, with Italian as an option (#11).
- Profiles (account and server), avatars, server icons (#13).
- Token saved in the system keychain instead of a plain file; an old token
  file is moved and deleted (#14).
- Avatar in the profile card, users highlighted on hover, pictures in the
  direct messages list (#15).
- Per-channel permissions: hidden channels, read-only channels, voice
  channels that cannot be joined, microphone only where speaking is
  allowed (#16).
- Lighter GIFs: frames only while on screen, redraws on a common grid (#17).
- App icon: a little gardener watering a flower, also on the installer (#18).

## 0.1.0

- First version: graphical (iced) and terminal interfaces, login with a QR
  code or a token, gateway, messages, Windows installer, automatic security
  checks (#1).
