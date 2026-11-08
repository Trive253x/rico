# Changelog

Every implementation changes the version (see AGENTS.md, rule 7): a new
feature bumps the minor number (0.2.0 → 0.3.0), a fix or improvement the
patch number (0.2.0 → 0.2.1). Numbers in brackets are pull requests.

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
