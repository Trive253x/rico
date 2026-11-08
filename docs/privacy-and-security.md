# Privacy and security

## Token

- It is obtained with the QR login (the password never goes through
  ferrocord) or by pasting it; in the second case it is checked before being
  saved.
- It is saved in a file readable only by the user (permissions 600 on Linux
  and macOS, profile folder on Windows). `ferrocord --logout` deletes it.
- In HTTP requests the `Authorization` header is marked as sensitive, so it
  does not end up in the libraries' debug logs.
- It is never written to the log, nor shown in the interface.

## Log

The log contains only technical events: connections, errors, numbers
(servers, messages, download sizes, audio packets). It does not contain
tokens, message text, sent links or channel contents.

## Contacted servers

| Server | Why |
|---|---|
| `gateway.discord.gg`, `discord.com` | gateway and API |
| `remote-auth-gateway.discord.gg` | QR login |
| `cdn.discordapp.com`, `media.discordapp.net`, `images-ext-1/2.discordapp.net` | attachments, emojis, images and GIFs through Discord's proxy |
| `media.tenor.com`, `media1.tenor.com` | GIFs of Tenor links |
| `media.giphy.com`, `i.giphy.com`, `media0-4.giphy.com` | GIFs of Giphy links |
| `static.klipy.com` | previews in the GIF picker |
| `*.discord.media` voice servers (WebSocket and UDP) | voice chat, only after clicking a voice channel |

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

The interface language is saved in a `language` file next to the token
(`en` or `it`). It is read as plain text and anything else is ignored.

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
