# ferrocord

An alternative Discord client written in Rust: native, lightweight, without
Electron or Chromium. Two interfaces on the same data:

- **`ferrocord`**: graphical interface (iced), the one the installer installs;
- **`ferrocord-tui`**: terminal interface (ratatui).

The interface is in English by default and can be switched to Italian (button
in the top bar or on the login screen, or `--lang it`).

> **Warning.** This is an **unofficial** client. Discord's Terms of Service
> forbid accessing the service with third-party clients and automating user
> accounts: using it can get the account closed. For development and testing
> use the **demo mode** or a **secondary account**, and never add automation
> (automatic sending, scraping, self-bots).

## Documentation

| Document | Contents |
|---|---|
| [docs/architecture.md](docs/architecture.md) | modules, data flow, design choices |
| [docs/discord-protocol.md](docs/discord-protocol.md) | gateway, REST, QR login, media, GIFs, voice: what ferrocord uses and how |
| [docs/development.md](docs/development.md) | building, trying, tests, CI, branches and conventions |
| [docs/privacy-and-security.md](docs/privacy-and-security.md) | token, logs, contacted servers, automatic checks |
| [docs/todo.md](docs/todo.md) | things to implement, in order |
| [CHANGELOG.md](CHANGELOG.md) | what changed in each version |
| [AGENTS.md](AGENTS.md) | instructions for AI assistants working on the code |

## Current state

| Feature | State |
|---|---|
| Login with a QR code (Discord app on the phone) or with a token | ✅ |
| Token saved in the system keychain | ✅ |
| Gateway: identify, heartbeat, resume, reconnection with backoff, `zlib-stream` | ✅ |
| Servers, categories, text channels, direct messages, unread | ✅ |
| History, loading older messages | ✅ |
| Real-time messages, edits, deletions, sending | ✅ |
| Readable mentions, channels and timestamps; replies, reactions, "is typing…" | ✅ |
| Previews of attached images, full-screen view with zoom, saving | ✅ |
| Animated GIFs: Tenor, Giphy and Klipy links (all through Discord's proxy), attached GIFs | ✅ |
| GIF picker (Discord's search) and emoji picker (Unicode and server emojis) | ✅ |
| Server emojis in messages and reactions, usable only in their own server | ✅ |
| Voice chat: who is in voice channels, connection to the voice server, encrypted audio | ✅ |
| Voice chat: Opus audio, microphone and speakers, DAVE end-to-end encryption | ✅ tested on Discord (receiving and sending) |
| Avatars (account and server profile), server icons, pictures in the direct messages list | ✅ |
| Go Live streams: LIVE badge and stream preview | ✅ |
| Watching Go Live streams (video and sound) | ✅ tested on Discord, 1280×720 at about 30 fps |
| Stream window like Discord's: people of the channel on the left, full screen | ✅ tested in the demo |
| User profiles: account and server profile (banner, bio, pronouns, roles, member since) | ✅ |
| English or Italian interface | ✅ |
| Per-channel permissions: hidden channels, read-only channels, voice channels you cannot join | ✅ |
| Server folders from the Discord account, with an editor (saved on the account) | ✅ |
| Settings window: themes, zoom, compact messages, clock, animations, images, audio devices and levels | ✅ |
| Member list on the right of server channels (groups, online status, role colours) | ✅ tested on Discord |
| Per-person volume and local mute in calls | ✅ saving tested on Discord; the sound in a real call still to try |
| Moderation from the user card (mute, deafen, move, disconnect, timeout, kick, ban) with permissions and confirmation | ✅ tested in the demo |
| Replying, reacting, uploading files from the interface | ⏳ |
| Animated avatars, previews of normal links, bot "card" messages | ⏳ |
| Demo mode without an account (`--demo`) | ✅ |
| Windows installer and checks run locally before every release (`scripts/ci-local.ps1`) | ✅ |

**Voice chat.** Audio is encoded with Opus (libopus) and end-to-end encrypted
with DAVE, which Discord has required since 1 March 2026: if the server does
not enable DAVE the call is closed, never continued without end-to-end
encryption. The microphone opens only with the microphone button and is
released by turning it off, turning the sound off or leaving. Microphone and
speakers are Windows only for now.

**Streams.** Go Live video is H.264, decoded by OpenH264 (Cisco's decoder,
built from source with the program). H.264 is covered by patents: building
OpenH264 from source does not come with Cisco's patent licence, which applies
only to the binaries Cisco distributes. Watching streams works on every
system; their sound, like voice chat, only on Windows for now. Tested with Discord's servers (receiving,
with the privacy code matching the official client) and with `--demo` (local
fake voice server with real DAVE participants).

## To implement

The full list, in order and with the reasons, is in
[docs/todo.md](docs/todo.md). In short:

- **Rules and security**: previews from Tenor, Giphy and Klipy only after
  turning them on; the server column computed outside the drawing; the
  voice server host in the log.
- **To try on real Discord**: per-person volume in a call, moderation
  (only on your own server), the stream window on a real stream.
- **Features**: reply, react and upload files; Windows notifications and
  mention badges; edit and delete your messages; link previews, bot cards,
  spoilers, animated avatars; your status; friends, new direct messages,
  invites; push-to-talk, noise suppression, verification code per person;
  threads, forums, pins, search; stickers and bot commands; your own stream
  and webcam; server management; the terminal interface at the level of the
  graphical one.
- **Releases**: signed installer, Linux and macOS builds.

## Building

Stable Rust ≥ 1.85 is needed ([rustup.rs](https://rustup.rs)). On Windows also
the Visual Studio Build Tools (the Rust installer asks for them), with the
CMake component: it builds libopus.

```sh
cargo run --release                          # GUI
cargo run --release -- --demo                # GUI with fake data, no account
cargo run --release --bin ferrocord-tui      # terminal
cargo run --release --bin ferrocord-tui -- --demo
cargo test                                   # all tests (protocol, voice, interface)
```

The binary ends up in `target/release/ferrocord` (`.exe` on Windows). More
details in [docs/development.md](docs/development.md).

## Usage

At startup the login screen appears:

- **QR code** (recommended): open the Discord app on the phone → Settings →
  *Scan QR Code* and confirm. The password never goes through the client.
- **Token**: paste the token and confirm. The token is checked before being
  saved.

In the GUI:

- click a **photo** to open it large (mouse wheel to zoom, Esc to close) and
  save it to the Downloads folder;
- **GIF** and **🙂** buttons next to the message field: click a GIF to send
  it, click an emoji to insert it; server emojis can also be typed as
  `:name:`. The GIF picker shows only the titles until you turn on its
  previews in Settings → Text & images: they come from Klipy, which then
  sees your IP address;
- click a **voice channel** to join; in the panel at the bottom left the
  round buttons are the microphone (off when joining), the sound (headphones:
  silences everything) and leave (red). A muted button is red; hovering one
  says its state and what a click does.
  The 30-digit code next to the padlock is the call's privacy code: it is the
  same for every participant and can be compared to check that nobody is in
  between;
- the red **LIVE** badge next to someone in a voice channel means they are
  streaming (Go Live): with the mouse over them a small preview of the stream
  appears (like on Discord) and a click on them watches it (right click: the
  profile); the badge opens a larger preview, a still image Discord updates
  every few minutes. **Watch** opens the stream full window (Discord
  allows it only from inside the voice channel: if you are not there,
  ferrocord joins it with the microphone off, and everyone in the channel
  sees you). On the left the people of the channel, like on Discord: who is
  speaking (green ring), who has the microphone or the sound off, and a LIVE
  badge on whoever else is streaming (a click switches to their stream); the
  people button above the video hides them. Under the video: microphone,
  sound, the stream's volume (0 to 200%, also with ↑ ↓; a click on the
  speaker mutes the stream), full screen (also F11 or a double click on the
  video; in full screen the controls appear with the mouse at the bottom),
  "Stop watching" (or Esc: you stay in the channel; in full screen Esc
  leaves it first) and leave the channel;
- in a server channel the **members** are on the right, like on Discord
  (groups by role and online/offline, status dot, names in their role's
  colour); the people button in the channel header hides or shows them;
- click an **avatar or a name** (in messages, in voice channels, in the
  member list, your own at the bottom left) to open the user card: in a
  server it opens on the server profile (nickname, server avatar, roles),
  with a tab for the account one. In the card: that person's **volume in
  calls** (0–200%, or muted, only for you, kept for the next calls),
  **Mention** and **Copy ID**, and — only where your permissions and roles
  allow it — **moderation**: server mute/deafen, move to another voice
  channel or disconnect, timeout, kick and ban (kick, ban and timeout ask
  for a confirmation in the card first);
- the **EN / IT** button in the top bar switches the language (saved for the
  next start);
- servers are grouped in your Discord **folders**: click a folder to open it;
  the folder button under the servers (or a right click on a server) opens
  the editor to move servers, create, rename, colour, order and ungroup
  folders. "Save" changes them on your Discord account, so on every device;
- the **gear** next to your name at the bottom left (or Ctrl + ,) opens the
  settings: theme (dark, light, black, like the system, purple, light brown,
  blue, red), zoom, compact messages, clock, still GIFs, images and
  reactions on or off, microphone and speakers, volumes and microphone
  sensitivity, language, shortcuts. Esc closes them.

### Options and environment variables

| | |
|---|---|
| `--demo` | fake data, no connection to Discord |
| `--demo-login` | only the login screen, with a fake QR code |
| `--lang en\|it` | interface language (saved for the next start) |
| `--logout` | delete the saved token |
| `FERROCORD_TOKEN` | use this token instead of the saved one |
| `FERROCORD_LANG` | interface language for this run only (`en`, `it`) |
| `FERROCORD_LOG` | log level, e.g. `debug` or `warn,ferrocord=debug` |

The token is saved in the system keychain, never in a plain file:

| System | Where |
|---|---|
| Windows | Credential Manager, entry `discord-token.ferrocord` |
| macOS | Keychain, service `ferrocord` |
| Linux | kernel keyring (keyutils): it lasts until reboot, then you sign in again |

If the keychain cannot be used, the session goes on without saving the token
(you will have to sign in again next time) and the interface says so. A token
saved in a file by an older version is moved into the keychain at the first
start and the file is deleted.

The chosen language is saved in a file called `language`, the other settings
in `settings.json`, both in the configuration folder
(`%APPDATA%\ferrocord\ferrocord\config` on Windows).

The log goes to a file (`%LOCALAPPDATA%\ferrocord\ferrocord\data\ferrocord.log`
on Windows, `~/.local/share/ferrocord/ferrocord.log` on Linux). Tokens and
message text are never written to the log.

### Keys (terminal)

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | next / previous panel |
| `↑` `↓` (or `k` `j`) | move between servers, channels, messages |
| `Enter` | open server or channel, send message |
| `Esc` / `←` | back to the previous panel |
| `PgUp` / `PgDn` | scroll messages (at the top it loads older ones) |
| `End` | back to the latest messages |
| `Ctrl+W` / `Ctrl+U` | delete word / line in the text field |
| `Ctrl+X` | sign out |
| `Ctrl+C` | quit |

## Checks and releases

Builds, tests, security checks and the installer run on the development PC
with `scripts/ci-local.ps1` (the GitHub Actions build workflows are off).
Releases are published as `v<version>` with the installer and its SHA-256.

| Check | When | What it does |
|---|---|---|
| **Local CI** (`scripts/ci-local.ps1`) | before every merge into `main` | formatting, clippy, tests, release build, `cargo audit`, `cargo deny`, installer; with `-Publish` the GitHub release |
| **Sicurezza** (GitHub) | every push and every Monday | `cargo audit`, `cargo deny`, Semgrep, Gitleaks; if it finds important issues it opens an issue and sends an email |

Details in [docs/development.md](docs/development.md) and
[docs/privacy-and-security.md](docs/privacy-and-security.md). The installer is
not digitally signed: on first start Windows SmartScreen may ask for
confirmation (*More info → Run anyway*).

## Icon

`assets/icon.svg` is the source of the app icon (a little gardener watering a
flower). `python3 assets/make_icon.py` (needs `rsvg-convert` and Pillow)
writes `icon.png` (window icon), `icon.ico` and `icon.res`, which `build.rs`
links into the Windows `.exe` files.

## License

MIT
