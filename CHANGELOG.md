# Changelog

Every implementation changes the version (see AGENTS.md, rule 7): a new
feature bumps the minor number (0.2.0 → 0.3.0), a fix or improvement the
patch number (0.2.0 → 0.2.1). Numbers in brackets are pull requests.

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
