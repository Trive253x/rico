# Changelog

Every implementation changes the version (see AGENTS.md, rule 7): a new
feature bumps the minor number (0.2.0 → 0.3.0), a fix or improvement the
patch number (0.2.0 → 0.2.1). Numbers in brackets are pull requests.

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
