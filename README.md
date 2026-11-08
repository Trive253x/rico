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
| Animated GIFs: Tenor, Giphy and Klipy links, attached GIFs | ✅ |
| GIF picker (Discord's search) and emoji picker (Unicode and server emojis) | ✅ |
| Server emojis in messages and reactions, usable only in their own server | ✅ |
| Voice chat: who is in voice channels, connection to the voice server, encrypted audio | ✅ |
| Voice chat: Opus audio, microphone and speakers, DAVE end-to-end encryption | ✅ receiving tested on Discord; sending tested in the demo |
| Avatars (account and server profile), server icons | ✅ |
| User profiles: account and server profile (banner, bio, pronouns, roles, member since) | ✅ |
| English or Italian interface | ✅ |
| Per-channel permissions (channels you cannot access are shown too) | ⏳ |
| Replying, reacting, uploading files from the interface | ⏳ |
| Real avatars, previews of normal links, bot "card" messages | ⏳ |
| Demo mode without an account (`--demo`) | ✅ |
| Windows installer on every push, automatic security checks | ✅ |

**Voice chat.** Audio is encoded with Opus (libopus) and end-to-end encrypted
with DAVE, which Discord has required since 1 March 2026: if the server does
not enable DAVE the call is closed, never continued without end-to-end
encryption. The microphone opens only with the "Microphone" button and is
released by turning it off, turning the sound off or leaving. Microphone and
speakers are Windows only for now. Tested with Discord's servers (receiving,
with the privacy code matching the official client) and with `--demo` (local
fake voice server with real DAVE participants).

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
  `:name:`;
- click a **voice channel** to join; in the panel at the bottom left
  "Microphone" (off when joining), "Sound" to silence everything and "Leave".
  The 30-digit code next to the padlock is the call's privacy code: it is the
  same for every participant and can be compared to check that nobody is in
  between;
- click an **avatar or a name** (in messages, in voice channels, your own at
  the bottom left) to open the profile: in a server it opens on the server
  profile (nickname, server avatar, roles), with a tab for the account one;
- the **EN / IT** button in the top bar switches the language (saved for the
  next start).

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

The chosen language is saved in a file called `language` in the configuration
folder (`%APPDATA%\ferrocord\ferrocord\config` on Windows).

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

## GitHub automation

| Workflow | When | What it does |
|---|---|---|
| **CI** | every push | formatting, clippy, tests and builds on Windows, macOS and Linux; screenshot of the demo GUI |
| **Installer Windows** | every push | Inno Setup installer (artifact); on `main` it updates the **nightly** release |
| **Sicurezza** (security) | every push and every Monday | `cargo audit`, `cargo deny`, Semgrep, Gitleaks; if it finds important issues it opens an issue and sends an email |

Details in [docs/development.md](docs/development.md) and
[docs/privacy-and-security.md](docs/privacy-and-security.md). The installer is
not digitally signed: on first start Windows SmartScreen may ask for
confirmation (*More info → Run anyway*).

## License

MIT
