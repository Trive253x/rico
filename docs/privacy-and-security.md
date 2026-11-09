# Privacy and security

## Token

- It is obtained with the QR login (the password never goes through
  ferrocord) or by pasting it; in the second case it is checked before being
  saved.
- It is saved in the operating system keychain (Windows Credential Manager,
  macOS Keychain, Linux kernel keyring until reboot), never in a plain file.
  If the keychain does not work, the session continues without saving it.
- A token left in a file by an older version is moved into the keychain at
  startup and the file is overwritten and deleted; if the keychain does not
  work the file stays until the next attempt (nothing new is written in clear).
- Logging out (or `ferrocord --logout`) deletes it from the keychain, and the
  old file too if it is still there.
- In HTTP requests the `Authorization` header is marked as sensitive, so it
  does not end up in the libraries' debug logs.
- It is never written to the log, nor shown in the interface.
- It is sent only to Discord: REST and QR login never follow redirects and
  use HTTPS only; the gateway resumes on the address Discord gives only if it
  is a `wss://…discord.gg` address, otherwise on the default one.

## Log

The log contains only technical events: connections, errors, numbers
(servers, messages, download sizes, audio packets). It does not contain
tokens, message text, sent links or channel contents. A malformed gateway
event is logged only with the kind and position of the error, since the
JSON library's message can quote values from the payload.

The gateway's compressed stream has limits (32 MB per compressed message,
128 MB decompressed): beyond them the connection is dropped and opened again.

In the terminal interface, control characters received in any text (names,
titles, messages) are removed before drawing: they cannot send commands to
the terminal.

## Contacted servers

| Server | Why |
|---|---|
| `gateway.discord.gg`, `discord.com` | gateway and API |
| `remote-auth-gateway.discord.gg` | QR login |
| `cdn.discordapp.com`, `media.discordapp.net`, `images-ext-1/2.discordapp.net` | attachments, emojis, images and GIFs through Discord's proxy |
| `media.tenor.com`, `media1.tenor.com` | GIFs of Tenor links |
| `media.giphy.com`, `i.giphy.com`, `media0-4.giphy.com` | GIFs of Giphy links |
| `static.klipy.com` | previews in the GIF picker |
| `*.discord.media` voice servers (WebSocket and UDP) | voice chat and streams, only after clicking a voice channel or "Watch" |

`media::fetch` downloads only from these servers (HTTPS), with size limits
(8 MB for previews, 25 MB for the large view, 600 MB for saved files).

**Your IP address is visible to third parties in two cases**, because Discord
does not offer a copy through its proxy:

- GIFs of Tenor and Giphy links are downloaded from Tenor's (Google) and
  Giphy's servers;
- previews in the GIF picker are downloaded from Klipy.

Klipy GIFs sent in messages go through Discord's proxy instead.

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
  badge (and again only with "Refresh", at most every 15 s), never in the
  background. It comes from Discord's API and CDN; the streamer is not told.
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

## Downloaded files

- The file name is cleaned up (no paths, characters forbidden on Windows,
  reserved names) and made unique in the Downloads folder.
- The download goes to a `.part` file, renamed only when complete.

## Decoding images and animations

Limits against files built to eat memory: animated GIFs and WebPs at most
4096 px per side, 300 frames and 64 MB once decoded; decoding happens off the
interface thread.

## Voice chat

- Audio travels encrypted between ferrocord and Discord's voice server
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
- Audio, keys and payloads never end up in the log or on disk. `davey` and
  `openmls` logs are capped at `info`/`warn` even with `FERROCORD_LOG=trace`,
  because at `trace` level they would write secrets.
- The voice server address is accepted only as `wss://` with a valid host
  name; `ws://` only towards `127.0.0.1` (the demo's fake server).
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
message layout, animations, images, reactions, audio devices (by name),
volumes and microphone sensitivity. No token, message or personal data. It is
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
