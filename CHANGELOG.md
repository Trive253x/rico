# Changelog

Every implementation changes the version (see AGENTS.md, rule 7): a new
feature bumps the minor number (0.2.0 → 0.3.0), a fix or improvement the
patch number (0.2.0 → 0.2.1). Numbers in brackets are pull requests.

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
