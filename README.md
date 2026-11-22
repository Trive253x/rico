# Rico

An alternative Discord client written in Rust: native, lightweight, without
Electron or Chromium (called ferrocord up to version 0.12). Two interfaces on
the same data:

- **`rico`**: graphical interface (iced), the one the installer installs;
- **`rico-tui`**: terminal interface (ratatui).

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
| [docs/discord-protocol.md](docs/discord-protocol.md) | gateway, REST, QR login, media, GIFs, voice: what Rico uses and how |
| [docs/development.md](docs/development.md) | building, trying, tests, CI, branches and conventions |
| [docs/privacy-and-security.md](docs/privacy-and-security.md) | token, logs, contacted servers, automatic checks |
| [docs/todo.md](docs/todo.md) | things to implement, in order |
| [CHANGELOG.md](CHANGELOG.md) | what changed in each version |
| [AGENTS.md](AGENTS.md) | instructions for AI assistants working on the code |

## Current state

| Feature | State |
|---|---|
| Login with a QR code (Discord app on the phone), with email and password (with the two-factor code), or with a token | ✅ tested on Discord |
| Token saved in the system keychain | ✅ |
| Gateway: identify, heartbeat, resume, reconnection with backoff, `zlib-stream` | ✅ |
| Servers, categories, text channels, direct messages, unread | ✅ |
| History, loading older messages | ✅ |
| Real-time messages, edits, deletions, sending | ✅ |
| Readable mentions (role names; a click shows the person or the role's members), channels and timestamps; replies, reactions, "is typing…" | ✅ |
| Previews of attached images, full-screen view with zoom, saving | ✅ |
| Animated GIFs: Tenor, Giphy and Klipy links (all through Discord's proxy), attached GIFs | ✅ |
| GIF picker (Discord's search) and emoji picker (Unicode and server emojis) | ✅ |
| Server emojis in messages and reactions, usable only in their own server | ✅ |
| Voice chat: who is in voice channels, connection to the voice server, encrypted audio | ✅ |
| Voice chat: Opus audio, microphone and speakers, DAVE end-to-end encryption | ✅ tested on Discord (receiving and sending) |
| Avatars (account and server profile), server icons, pictures in the direct messages list | ✅ |
| Go Live streams: LIVE badge and stream preview | ✅ |
| Watching Go Live streams (video and sound) | ✅ tested on Discord, 1280×720 at about 30 fps |
| Stream window like Discord's: people of the channel on the left, full screen | ✅ tested on Discord |
| Streaming your screen or a window (Go Live), with the program's sound | ✅ tested in the demo and on Discord (watched from the official client) |
| User profiles: account and server profile (banner, bio, pronouns, roles, member since) | ✅ |
| English or Italian interface | ✅ |
| Per-channel permissions: hidden channels, read-only channels, voice channels you cannot join | ✅ |
| Server folders from the Discord account, with an editor (saved on the account) | ✅ |
| Settings window: themes, zoom, compact messages, clock, animations, images, audio devices and levels | ✅ |
| Member list on the right of server channels (groups, online status, role colours) | ✅ tested on Discord |
| Member list styles: classic, compact (with search), rich (what each person is doing), by activity | ✅ tested in the demo |
| Per-person volume and local mute in calls | ✅ tested on Discord |
| Voice indicators: microphone or sound off by the person (grey) or by the server (red), muted for you | ✅ from 0.18.0, tested in the demo |
| Windows notifications for direct messages and mentions (following the account's notification settings), mention badges on servers, folders, channels and DMs, read positions in sync with other devices | ✅ from 0.19.0 |
| Push-to-talk (also with Rico in the background), noise suppression to choose (Standard, RNNoise, DeepFilterNet, DPDFNet), echo cancellation, automatic gain, verification code per person | ✅ from 0.19.0 |
| Moderation from the user card (mute, deafen, move, disconnect, timeout, kick, ban) with permissions and confirmation | ✅ tested on Discord |
| Replying, reacting, uploading files (drag and drop), editing and deleting your messages, going to the replied message | ✅ tested on Discord |
| Attach button with the system's file dialog | ✅ |
| Right-click menu on messages (react, reply, edit, copy text, link or ID, delete) | ✅ |
| Right-click menu on people (profile, mention, message, mute for you, moderation, copy name or ID) | ✅ tested in the demo |
| Right-click menu on server icons (mark as read, mute, folders, copy ID, leave) | ✅ from 0.19.0 |
| In-app updates (signed, no downgrades, Restart now; "Check now" in Settings › About from 0.19.0) | ✅ from 0.18.0 |
| Your status (online, idle, do not disturb, invisible) and custom status, saved on the account | ✅ |
| Friends: online and all (with status and activity), friend requests received and sent (accept, decline, cancel), blocked people (unblock), sending a request by username | ✅ from 0.18.0, tested in the demo |
| Animated avatars, previews of normal links, bot "card" messages | ⏳ |
| Demo mode without an account (`--demo`) | ✅ |
| Windows installer and checks run locally before every release (`scripts/ci-local.ps1`) | ✅ |

**Voice chat.** Audio is encoded with Opus (libopus) and end-to-end encrypted
with DAVE, which Discord has required since 1 March 2026: if the server does
not enable DAVE the call is closed, never continued without end-to-end
encryption. The microphone opens only with the microphone button and is
released by turning it off, turning the sound off or leaving. Microphone and
speakers are Windows only for now. With push-to-talk on, the open
microphone sends only while the key is held. Noise suppression (Standard
by default; RNNoise, DeepFilterNet or DPDFNet for keyboard and other
sudden noise; DPDFNet's model is downloaded once from Hugging Face when
chosen),
echo cancellation (on by default) and automatic gain clean the microphone
on your computer before it is encoded.

**Streams.** Go Live video is H.264, decoded by OpenH264 (Cisco's decoder,
built from source with the program). H.264 is covered by patents: building
OpenH264 from source does not come with Cisco's patent licence, which applies
only to the binaries Cisco distributes. Watching streams works on every
system; their sound, like voice chat, only on Windows for now. Streaming your
own screen is Windows only (Windows Graphics Capture for the picture, WASAPI
process loopback for the sound). Tested with Discord's servers (receiving,
with the privacy code matching the official client) and with `--demo` (local
fake voice server with real DAVE participants).

## Known problem: voice timing on Windows

The voice packets leave on a 20 ms timer. On Windows its precision depends on
the system timer resolution, which other programs (Discord, browsers, games)
often raise to 1 ms for everyone; when none does, it stays at about 15.6 ms.
Then the timer often fires late and the voice leaves in small bursts, which
some listeners (consoles above all) hear as lag or gaps. The log counts it
once a minute while the microphone is on (`voice: microphone: … late timer
ticks`): normally 1–7 a minute, up to 120 with DeepFilterNet and 300 with
DPDFNet in a demo test on 22 November 2026, with the PC otherwise idle.

Not fixed yet. The planned fix is to ask Windows for 1 ms resolution during
calls (`timeBeginPeriod`, as Discord and browsers do), which needs a direct
Windows API call (FFI) and the owner's approval. Meanwhile, the lighter noise
suppressions (Standard, RNNoise) are the least affected.

## Compared with the official client

Measured on the owner's PC (Windows 11, 1920×1080) on 15 November 2026,
both clients signed in to the same account with the same servers, Rico
0.12.0 against Discord 1.0.9268:

| | Rico | Discord (official) |
|---|---|---|
| Processes | 1 | 6 |
| Memory in use (working set) | 159 MB | 1,114 MB |
| Private memory | 282 MB | 1,189 MB |
| Space on disk | 39.5 MB, installed by the installer (GUI 28 MB, terminal 7 MB, uninstaller 4 MB) | 590 MB (`AppData\Local\Discord`) |
| Installer | 12 MB | — |
| From start to connected (gateway ready) | 0.7 s | — |
| CPU, open on a text channel and idle | 0.1% of one core | — |

Rico was open on a server's text channel with images and a GIF; for
Discord the figures are those of the running client as the owner was using
it. Not measured ("—"): Discord's installer, its start and its idle CPU,
which would need closing and restarting it.

## To implement

The full list, in order and with the reasons, is in
[docs/todo.md](docs/todo.md). In short:

- **Features**: link previews, bot cards,
  spoilers, animated avatars; your status; blocking people, nicknames for
  friends, invites;
  threads, forums, pins, search; stickers and bot commands; the webcam;
  server management; the terminal interface at the level of the
  graphical one.
- **Releases**: signed installer, Linux and macOS builds.

## Building

Stable Rust ≥ 1.88 is needed ([rustup.rs](https://rustup.rs)). On Windows also
the Visual Studio Build Tools (the Rust installer asks for them), with the
CMake component: it builds libopus. NASM on the PATH makes OpenH264 (stream
video) use its assembly: without it the build works but video is several
times slower.

```sh
cargo run --release                          # GUI
cargo run --release -- --demo                # GUI with fake data, no account
cargo run --release --bin rico-tui      # terminal
cargo run --release --bin rico-tui -- --demo
cargo test                                   # all tests (protocol, voice, interface)
```

The binary ends up in `target/release/rico` (`.exe` on Windows). More
details in [docs/development.md](docs/development.md).

## Usage

At startup the login screen appears:

- **QR code** (recommended): open the Discord app on the phone → Settings →
  *Scan QR Code* and confirm. The password never goes through the client.
- **Email**: email (or phone) and password, then the code of the
  authenticator app if the account has two-factor authentication. The
  password goes only to Discord and is not saved. If Discord sends an email
  to authorize a new login location, approve it and sign in again.
- **Token**: paste the token and confirm. The token is checked before being
  saved.

In the GUI:

- click a **photo** to open it large (mouse wheel to zoom, Esc to close) and
  save it to the Downloads folder;
- **GIF** and **emoji** buttons (the two icons next to the message field): click a GIF to send
  it, click an emoji to insert it; server emojis can also be typed as
  `:name:`. The GIF picker shows only the titles until you turn on its
  previews in Settings → Text & images: they come from Klipy, which then
  sees your IP address;
- click a **mention**: `@person` opens their profile, `@role` the members
  with that role that Rico has loaded so far;
- with the mouse over a message a small bar of icons appears (the name shows
  under the mouse): **React** (opens the emoji picker: the emoji becomes a
  reaction), **Reply**, and for your own messages **Edit** and **Delete**
  (asks for a confirmation). A click on a reaction adds or removes yours. Esc
  cancels a reply or an edit. A click on the line above a reply goes to the
  message it answers;
- a **right click** on a message opens the same actions as a menu, plus
  **Copy text**, **Copy message link** and **Copy message ID**;
- a **right click** on a person (name or avatar in the messages, member list,
  voice channels) opens their menu, like on Discord: **Profile**,
  **Mention**, **Message** (opens or starts your direct messages),
  **Mute (only for you)**, the moderation your permissions allow (server
  mute or deafen and disconnect when they are in voice, **Timeout**, **Kick**,
  **Ban**: these ask in a small window where the menu was), **Copy
  username** and **Copy user ID**;
- **Friends** at the top of the direct messages: **Online** and **All**
  (status and what they are doing; **Message** opens your conversation,
  **Remove** asks first), **Pending** (requests received: **Accept** or
  **Decline**; sent: **Cancel request**; the red number counts the ones
  waiting for you), **Blocked** (**Unblock**) and **Add friend** (their
  username, not the display name). A click on a person opens their profile,
  a right click their menu. If Discord asks for a captcha Rico says so and
  sends nothing: do that action once in the official client;
- **drag files** onto the window, or use the **paperclip** left of the
  message field, to attach them to the next message (at most 10 files and
  10 MB); they are sent only when you press Send;
- a click on **your name** at the bottom left opens your status: online,
  idle, do not disturb or invisible, and a custom status with when it should
  go away. Both are saved on your Discord account, so every device shows
  them; the dot on your avatar shows the status;
- click a **voice channel** to join; in the panel at the bottom left the
  round buttons are the microphone (off when joining), the sound (headphones:
  silences everything) and leave (red). A muted button is red; hovering one
  says its state and what a click does. Next to each person in voice: a grey
  microphone or headphones crossed out when they turned them off themselves,
  red on a red background when a moderator did it in the server, and a
  crossed-out speaker when you muted them only for you.
  The 30-digit code next to the padlock is the call's privacy code: it is the
  same for every participant and can be compared to check that nobody is in
  between. To check one person, right click on them › **Verify end-to-end
  encryption**: the 45-digit code is the same on their side if nobody is in
  between. Alone in the channel there is no code yet: end-to-end encryption
  starts when someone joins (until then Rico sends nothing, as nobody would
  receive it). Push-to-talk, noise suppression, echo cancellation and
  automatic gain are in Settings › Voice & video;
- the red **LIVE** badge next to someone in a voice channel means they are
  streaming (Go Live): with the mouse over them a small preview of the stream
  appears (like on Discord) and a click on them watches it (right click:
  their menu); the badge opens a larger preview, a still image Discord updates
  every few minutes. **Watch** opens the stream full window (Discord
  allows it only from inside the voice channel: if you are not there,
  Rico joins it with the microphone off, and everyone in the channel
  sees you). On the left the people of the channel, like on Discord: who is
  speaking (green ring), who has the microphone or the sound off, and a LIVE
  badge on whoever else is streaming (a click switches to their stream); the
  people button above the video hides them. Under the video: microphone,
  sound, the stream's volume (0 to 200%, also with ↑ ↓; a click on the
  speaker mutes the stream), full screen (also F11 or a double click on the
  video; in full screen the controls appear with the mouse at the bottom),
  "Stop watching" (or Esc: you stay in the channel; in full screen Esc
  leaves it first) and leave the channel;
- in a voice channel the **screen button** (next to the sound) streams your
  screen or a window: choose what, the quality and whether the sound goes
  too, then **Go Live**. Nothing is captured before that click. While you
  stream, the panel shows **LIVE**, a small preview of what you send and
  **Stop**; leaving the channel also stops it;
- in a server channel the **members** are on the right, like on Discord
  (groups by role and online/offline, status dot, names in their role's
  colour); the people button in the channel header hides or shows them.
  Settings › Appearance › Member list changes how they are shown: classic,
  compact (short rows and a search), rich (what each person is doing) or by
  activity (who is in voice, who wrote here recently, online, offline);
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
  the folder button under the servers (or a right click on a folder) opens
  the editor to move servers, create, rename, colour, order and ungroup
  folders. "Save" changes them on your Discord account, so on every device;
- a **right click on a server** opens its menu: mark as read, mute or
  unmute it, the folder editor, copy its ID, leave it (after a
  confirmation). Red numbers on servers, folders, channels and the direct
  messages button count the mentions waiting for you;
- the **gear** next to your name at the bottom left (or Ctrl + ,) opens the
  settings: theme (dark, light, black, like the system, purple, light brown,
  blue, red, forest, ocean, sunset, graphite, Rosé Pine, Solarized light,
  mint), zoom, compact messages, clock, still GIFs, images and
  reactions on or off, microphone and speakers, volumes and microphone
  sensitivity, language, shortcuts. Esc closes them.

### Options and environment variables

| | |
|---|---|
| `--demo` | fake data, no connection to Discord |
| `--demo-login` | only the login screen, with a fake QR code |
| `--lang en\|it` | interface language (saved for the next start) |
| `--logout` | delete the saved token |
| `RICO_TOKEN` | use this token instead of the saved one |
| `RICO_LANG` | interface language for this run only (`en`, `it`) |
| `RICO_LOG` | log level, e.g. `debug` or `warn,rico=debug` |

The old names (`FERROCORD_TOKEN`, `FERROCORD_LANG`, `FERROCORD_LOG`) still
work.

The token is saved in the system keychain, never in a plain file:

| System | Where |
|---|---|
| Windows | Credential Manager, entry `discord-token.rico` |
| macOS | Keychain, service `rico` |
| Linux | kernel keyring (keyutils): it lasts until reboot, then you sign in again |

If the keychain cannot be used, the session goes on without saving the token
(you will have to sign in again next time) and the interface says so. A token
saved in a file by an older version is moved into the keychain at the first
start and the file is deleted.

**Coming from ferrocord** (the name up to 0.12): at the first start Rico
moves the old settings and log folders (`…\ferrocord\ferrocord\…`) to the
new ones and the saved token from the `ferrocord` keychain entry to the
`rico` one, so you stay signed in with the same settings. The installer
updates the old installation in place and removes `ferrocord.exe`,
`ferrocord-tui.exe` and the old shortcuts.

The chosen language is saved in a file called `language`, the other settings
in `settings.json`, both in the configuration folder
(`%APPDATA%\rico\rico\config` on Windows).

The log goes to a file (`%LOCALAPPDATA%\rico\rico\data\rico.log`
on Windows, `~/.local/share/rico/rico.log` on Linux), at most 20 MB per
run; one bigger than 5 MB at start is moved to `rico.old.log`. Tokens and
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
