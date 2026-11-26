# Privacy and security

## Token

- It is obtained with the QR login (the password never goes through
  Rico), with email and password, or by pasting it; in the last case it
  is checked before being saved.
- With the email login the password and the two-factor code are sent only to
  `discord.com` over HTTPS (no redirects followed), stay in memory only until
  Discord answers (the fields are emptied whatever the answer, except when
  there is no answer at all) and are never saved, logged or shown. The log
  says only the kind of answer (signed in, code needed, new location,
  captcha, refused).
- It is saved in the operating system keychain (Windows Credential Manager,
  macOS Keychain, Linux kernel keyring until reboot), never in a plain file.
  If the keychain does not work, the session continues without saving it.
- A token left in a file by an older version is moved into the keychain at
  startup and the file is overwritten and deleted; if the keychain does not
  work the file stays until the next attempt (nothing new is written in clear).
- A token saved under the old name (`ferrocord`, up to 0.12) is copied to the
  `rico` keychain entry and the old entry is deleted once the copy is saved;
  it never passes through a file.
- Logging out (or `rico --logout`) deletes it from the keychain (under both
  names), and the old file too if it is still there.
- In HTTP requests the `Authorization` header is marked as sensitive, so it
  does not end up in the libraries' debug logs.
- It is never written to the log, nor shown in the interface.
- It is sent only to Discord: REST, QR and email login never follow redirects and
  use HTTPS only; the gateway resumes on the address Discord gives only if it
  is a `wss://…discord.gg` address, otherwise on the default one.

## Log

The log contains only technical events: connections, errors, numbers
(servers, messages, download sizes, audio packets). It does not contain
tokens, message text, sent links or channel contents. A malformed gateway
event is logged only with the kind and position of the error, since the
JSON library's message can quote values from the payload.

When a call or a stream connects, the log names the voice server's host
(for example `c-mxp08-1234.discord.media`), also when the address is refused:
only the host, cleaned of any character other than letters, digits, `.`, `-`
and `:` and cut at 80 characters, never the voice token.

The gateway's compressed stream has limits (32 MB per compressed message,
128 MB decompressed): beyond them the connection is dropped and opened again.

In the terminal interface, control characters received in any text (names,
titles, messages) are removed before drawing: they cannot send commands to
the terminal.

## Files you upload

- Only files dragged onto the window by you, sent only when you press Send;
  nothing is uploaded in the demo.
- Only regular files (not folders or devices), at most 10 files and 10 MB per
  message, checked before reading them; a file that grows while being read
  is refused. They stay in memory until sent or removed, and are dropped when
  another channel is opened.
- Discord gets only the file name, never the folder it was in. The content is
  sent as it is: Rico does not remove the metadata of photos (for
  example the position in the EXIF data of a phone photo).

## Contacted servers

| Server | Why |
|---|---|
| `gateway.discord.gg`, `discord.com` | gateway and API |
| `remote-auth-gateway.discord.gg` | QR login |
| `cdn.discordapp.com`, `media.discordapp.net`, `images-ext-1/2.discordapp.net` | attachments, emojis, images and GIFs through Discord's proxy (also the GIFs of Tenor and Giphy links, as MP4 clips) |
| `static.klipy.com` | previews in the GIF picker, only if turned on in the settings |
| `*.discord.media` voice servers (WebSocket and UDP) | voice chat and streams, only after clicking a voice channel, "Watch" or "Go Live" |
| `huggingface.co`, `*.hf.co` (its file CDN) | only when DPDFNet is chosen as noise suppression and its model is not on disk yet: one download of its 16 kHz model (8.7 MB) from a fixed revision, checked against a fixed SHA-256 before it is saved and every time it is read; kept in Rico's data folder (`models/`). Redirects followed by hand, only to these hosts; nothing is sent but the request |
| `github.com`, `objects.githubusercontent.com`, `release-assets.githubusercontent.com` | in-app updates (Windows): once a day the signed manifest of the newest release, and the installer when there is a newer one. On by default, switch in Settings › About; active only in builds with the owner's signing keys (see `docs/update-design.md`) |

`media::fetch` downloads only from these servers (HTTPS), with size limits
(8 MB for previews, 25 MB for the large view, 600 MB for saved files).

**No third party sees your IP address unless you choose so**, apart from
GitHub for the update check (on by default, the owner's choice; it can be
turned off in Settings › About). The other case is the previews of the GIF picker, which Discord does not offer
through its proxy: they come from Klipy, and only after turning on
"Previews in the GIF picker" (Settings → Text & images, off by default).
Choosing DPDFNet as noise suppression downloads its model once from
Hugging Face, which then sees your IP address (the settings say so).
Without them the picker shows the titles and sending GIFs works the same.

GIFs in messages never contact the GIF sites: for Tenor and Giphy links the
MP4 clip Discord's proxy serves is decoded on the PC (`video/mp4.rs` reads
the file, bounded and checked; OpenH264 decodes it, like stream video), and
Klipy GIFs come from Discord's proxy as animated images. Tenor and Giphy are
not in the list of allowed servers.

## Member list and moderation

- The member list of a server channel is asked on the gateway (op 37) when
  the channel is opened, as the official client does, and only while the
  list is shown; at most 5 pages of 100 rows. It is kept in memory only.
- Moderation (server mute/deafen, move, disconnect, timeout, kick, ban) is
  sent only after a click in the user card, and kick, ban and timeout only
  after "Confirm". The buttons appear only with the permissions and above
  the person in the roles; Discord checks again. Nothing is sent in the
  demo.

## Profiles, avatars and icons

- Avatars, server icons and profile banners come only from Discord's CDN
  (`cdn.discordapp.com`), like attachments: no other site sees your IP.
- A user's profile (`/users/{id}/profile`) is asked only when you open their
  card, and kept in memory for the session only; it is never written to disk.
- Hashes and IDs from the network are checked before building an address;
  images are decoded with size limits (4096 px per side, 64 MB) off the
  interface thread, and the bio shown is limited to 1000 characters.

## Stream previews

- The preview of a Go Live stream is asked only when you click the LIVE
  badge (and again only with "Refresh", at most every 15 s) or when the
  mouse goes over someone streaming (at most once a minute per stream, at
  most 32 streams kept), never in the background. It comes from Discord's API and CDN; the streamer is not told.
- The address Discord returns is accepted only on `cdn.discordapp.com`
  under `streams/`; the IDs in the stream key must be numeric.

## Watching streams

- Watching a stream means joining its voice channel: everyone in the channel
  sees you, and Discord shows the streamer who is watching. The microphone
  stays off; "Watch" only joins after a click, never on its own.
- The stream video is end-to-end encrypted with DAVE like audio, in a
  separate MLS group with the streamer; frames that do not decrypt are
  dropped.
- **Video decoding goes through C++ code (OpenH264, via FFI)**, fed with data
  sent by other people. To limit what a broken or hostile stream can do:
  decoding runs on its own thread with a queue of 4 frames (dropped beyond
  that, never queued without limit); RTP frames are capped at 4 MB and the
  reorder buffer at 2048 packets; pictures larger than 3840×2160 are refused
  before the RGBA buffer is allocated; a panic during decoding is caught and
  the decoder replaced. A memory bug inside OpenH264 itself cannot be stopped
  by these limits: keep the program up to date.
- Requests to the streamer's client (NACK, PLI) are limited: at most 3 per
  lost packet and one keyframe request every 2 s.
- No frame, picture, key or stream token ends up in the log
  (`STREAM_SERVER_UPDATE` hides the token even in debug output).

## Streaming your screen

- Nothing is captured until you pick a screen or a window and click
  **Go Live**. While something is captured, the call panel shows **LIVE**
  with a small preview of what is sent; stopping (button, leaving the
  channel, Discord ending the stream, the streamed window closing, quitting)
  stops the capture threads and releases the screen and the sound.
- Windows draws a yellow border around a captured item; Rico asks
  Windows to leave it out where it allows (Windows 11), and the LIVE line in
  Rico is the sign instead. On older Windows the border stays.
- The sound is captured per program with WASAPI process loopback: a window's
  program (with its child processes), or for a whole screen every program
  except Rico. The voices of the call are never in the stream.
- Picture and sound are end-to-end encrypted with DAVE before leaving, like
  the voice; nothing is sent before the encryption is ready.
- Capture goes through two crates that wrap the system's interfaces
  (`windows-capture`, `wasapi`, both on the `windows` crate cpal already
  uses), approved by the owner; Rico itself adds no `unsafe` code there
  (its only `unsafe` is the 1 ms system timer during calls,
  `src/audio/timer.rs`: `timeBeginPeriod`/`timeEndPeriod`, integer
  arguments, no memory shared, balanced one for one). The
  encoder (OpenH264) runs on its own thread with a one-place input and an
  output of at most 6 frames; a panic inside it is caught.
- Window titles appear only in the list you choose from: the log only gets
  counts, the kind of source and the quality.

## Downloaded files

- The file name is cleaned up (no paths, characters forbidden on Windows,
  reserved names) and made unique in the Downloads folder.
- The download goes to a `.part` file, renamed only when complete.

## Decoding images and animations

Limits against files built to eat memory: animated GIFs and WebPs at most
4096 px per side, 300 frames and 64 MB once decoded; decoding happens off the
interface thread.

## Notifications and read positions

- Which messages notify follows the account's notification settings
  (muted servers and channels, "only mentions"), plus: not for the channel
  you are looking at, not with the status Do not disturb, at most 5 every
  10 s. Turned off in Settings › Notifications; the message text can be left
  out of them.
- Windows keeps shown notifications in its notification centre, with their
  text if "Show the message text" is on. To name them "Rico", the first
  notification writes `HKCU\Software\Classes\AppUserModelId\Trive253x.Rico`
  (`DisplayName`, `IconUri`) and the icon `notification-icon.png` next to
  the log; uninstalling leaves that key (harmless, it can be deleted).
- Rico tells Discord how far you have read a channel (as the official client
  does, so your other devices agree): when you open it, or when a message
  arrives in it while the window has the focus; never twice for the same
  message, at most once every 3 s per channel. Nothing is sent in the demo.

## Voice chat

- Audio travels encrypted between Rico and Discord's voice server
  (AES-256-GCM or XChaCha20-Poly1305). Tampered packets or packets with the
  wrong key are dropped and counted.
- Inside the transport, every audio frame is **end-to-end** encrypted with
  DAVE: Discord cannot listen. If the server does not enable DAVE, or asks to
  go back to version 0, the call is closed; frames that do not decrypt with
  DAVE are dropped, never played.
- The 30-digit privacy code in the voice panel is the same for every
  participant: comparing it (by voice or elsewhere) confirms that nobody is in
  between. Verified against the official mobile client.
- **Microphone**: it opens only with a click on "Microphone", which stays red
  while it is on. Turning it off, turning the sound off, leaving the channel
  or closing the app closes the device. Nothing is sent until end-to-end
  encryption is ready.
- **Verification code per person** (right click › Verify end-to-end
  encryption): DAVE's pairwise fingerprint of your key and theirs, worked out
  only on that click; nothing is sent to anyone.
- **Microphone clean-up** (noise suppression, echo cancellation, automatic
  gain): done on your computer by `sonora` (pure Rust, no C), and for the
  stronger noise suppressors by `nnnoiseless` (RNNoise) or DPDFNet (run by
  `tract`; its model downloaded once from Hugging Face, see "Contacted
  servers"; no audio ever leaves the computer for it), only while the
  microphone is on; the echo canceller is given what the speakers play,
  kept only for a few tens of milliseconds in memory.
- **Push-to-talk**: the key is listened to only for that key. In Rico's
  window from its own keyboard events; on Windows, with Rico in the
  background, through `RegisterHotKey`, which tells Rico only when that key
  combination is pressed (no other keys are seen, no keyboard hook). It is
  registered only while you are in a call with push-to-talk on, and released
  when the call ends or push-to-talk is turned off. The key is saved in the
  settings file; no key presses are logged (only "registered" / "released").
- Audio, keys and payloads never end up in the log or on disk. `davey` and
  `openmls` logs are capped at `info`/`warn` even with `RICO_LOG=trace`,
  because at `trace` level they would write secrets.
- The voice server address (where the voice token is sent) is accepted only
  as `wss://` towards a `*.discord.media` host, the domain Discord uses for
  voice and stream servers; `ws://` only towards `127.0.0.1` (the demo's fake server).
- In the demo the fake voice server listens only on `127.0.0.1`; the speakers
  play synthetic voices and the microphone stays off until you turn it on.
- Opus is decoded by libopus (C, through the `opus` crate), with packets
  limited to 1275 bytes and at most 64 decoders at a time.

## Settings

The interface language is saved in a `language` file in the configuration
folder (`en` or `it`). It is read as plain text and anything else is ignored.

**Server folders** are the exception: they live on the Discord account, are
read after login and are changed on Discord only with the folder editor's
"Save" (field 14 of the settings only, with the version check, so a change
made meanwhile on another device is not overwritten). Nothing else in the
account settings is ever sent. The protobuf from Discord is decoded with
limits (size, number of folders and servers, name length) and a broken one
is ignored.

The other settings are in `settings.json` in the same folder: theme, zoom,
message layout, animations, images, reactions, member list shown or not,
audio devices (by name), volumes and microphone sensitivity, and the
volume chosen for single people in calls (by Discord user ID only: no
names; at most 1000, invalid IDs dropped). No token, message or other
personal data. It is
written to a temporary file and then renamed (never half written); a broken
file gives the defaults and values out of range are brought back into range.
Nothing in it is sent to Discord. Turning images off also stops their
downloads. Choosing a microphone never opens it: it still opens only with its
button.

## Automatic checks

The **Sicurezza** (security) workflow runs on every push and every Monday:

- `cargo audit`: known vulnerabilities in dependencies (RustSec database);
- `cargo deny`: licences, origin and duplicates of dependencies;
- Semgrep: static analysis of the Rust code and of the workflows;
- Gitleaks: tokens or passwords that ended up in the repository, history
  included.

If it finds **important issues** (a known vulnerability, a serious Semgrep
finding, a secret in the repository, a check that could not run) it opens or
updates an issue with the `sicurezza` label and makes the run fail: GitHub
sends an email for both. If the run is cancelled by a later push it reports
nothing (the checks start again on the new commit).

Minor warnings (unmaintained libraries, duplicates) end up in the run
summary. Advisories ignored on purpose are in `.cargo/audit.toml`, each with
its reason; the DAVE dependency ones (crypto-bigint, libcrux-*) are to be
reviewed at every new release of davey, openmls_rust_crypto or hpke-rs.
Gitleaks findings checked by hand are in `.gitleaksignore` (one fingerprint
each, with its reason): so far only the checksum of the `libcrux-secrets`
package in `Cargo.lock`, flagged because of the package name.
