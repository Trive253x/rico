# Architecture

Rico is a single library (`src/lib.rs`) with two programs on top: the GUI
(`src/main.rs`) and the terminal (`src/bin/rico-tui.rs`). The network
code and the data do not know which interface is using them.

## Modules

```
src/
├── main.rs                 `rico` program (GUI)
├── bin/rico-tui.rs         `rico-tui` program (terminal)
├── lib.rs                  command line options, startup, logging
├── i18n.rs                 interface language (English, Italian): t(), tf!
├── config.rs               paths, saved token, language and settings, log file
├── settings.rs             client settings (theme, zoom, layout, images, audio)
├── store.rs                session data, shared by the two interfaces
├── store/friends.rs        friends, friend requests, blocked people, friends' status
├── text.rs                 Discord tags (<@id>, <#id>, <:emoji:id>, <t:…>)
│                           made readable; `:name:` → server emoji when sending
├── permissions.rs          Discord permissions: what we can see and do in each
│                           channel (roles, overwrites, owner, timeout)
├── media.rs                downloads of images, GIFs and emojis: allowed
│                           servers, limits, animation decoding, saving files
├── demo.rs                 fake data for --demo
├── discord/
│   ├── gateway.rs          main WebSocket: identify/resume, heartbeat,
│   │                       events, commands (op 4 for voice)
│   ├── zlib.rs             zlib-stream decompression for the gateway
│   ├── rest.rs             REST API: profile, history, sending (replies,
│   │                       files), edits, deletions, reactions, GIF search
│   ├── folders.rs          server folders in the account settings (protobuf)
│   ├── remote_auth.rs      QR login (RSA-OAEP + ticket)
│   ├── password_login.rs   email and password login, two-factor code
│   ├── models.rs           data models, tolerant of missing fields
│   └── voice/
│       ├── mod.rs          connection to the voice server (voice gateway v8,
│       │                   UDP, ping, who is speaking, reconnection, audio)
│       ├── dave.rs         DAVE end-to-end encryption (opcodes 21–31)
│       ├── crypto.rs       encryption of audio packets (AES-GCM, XChaCha20)
│       ├── rtp.rs          RTP headers, IP discovery, RTCP NACK/PLI, RTX
│       └── mock.rs         local fake voice server (demo and tests)
├── gui/
│   ├── mod.rs              GUI state, messages, update and view
│   ├── style.rs            themes (colour palettes) and widget styles
│   ├── settings.rs         the settings window
│   ├── folders.rs          server folders: column and editor
│   ├── friends.rs          the Friends view: tabs, requests, add by username
│   ├── anim.rs             widget for animated GIFs
│   ├── picker.rs           GIF and emoji picker
│   ├── compose.rs          reply, edit and delete bars, files to upload,
│   │                       actions over the message under the mouse
│   ├── profile.rs          user profile card (account and server profile)
│   ├── role.rs             role card from a role mention: its members
│   ├── stream.rs           Go Live: LIVE badge, preview card, Watch button
│   ├── watch.rs            watching a stream: state, video widget, control bar
│   ├── golive.rs           our own stream: source picker, state, LIVE panel
│   ├── icons.rs            line icons for the voice and stream controls (SVG)
│   └── voice.rs            voice state, panel with microphone and privacy code
├── capture/
│   ├── mod.rs              what we stream: sources, test picture, Running
│   └── windows.rs          screen/window capture (windows-capture), program sound (wasapi)
├── video/
│   ├── h264.rs             H.264 ↔ RTP: reordering, FU-A/STAP-A, losses; packetizer
│   ├── decoder.rs          OpenH264 decoder thread
│   ├── encoder.rs          OpenH264 encoder thread for our stream (Quality, Outbox)
│   └── convert.rs          captured BGRA → scaled I420, preview
├── audio/
│   ├── codec.rs            Opus encoder and decoder (the only file using libopus)
│   ├── device.rs           microphone and speakers (cpal, Windows only)
│   ├── jitter.rs           jitter buffer per speaker and mixer (received voice)
│   └── dsp.rs              queues, resampling, voice activity detection
└── tui/                    terminal interface (ratatui)
```

## Data flow

```
             events                        iced messages
Discord ──► gateway.rs ──► mpsc channel ──► Gui::update ──► Store ──► view
   ▲                                          │
   └──────── rest.rs / gateway commands ◄─────┘  (async Tasks)
```

- **Gateway, QR login and voice** run as tokio tasks and send their events on
  a channel (`UnboundedSender<E>` with `E: From<Event>`). Each interface
  converts them into its own messages.
- In the **GUI** these tasks are iced *subscriptions*
  (`Subscription::run_with_id`): iced starts and stops them by itself when the
  state changes. The subscription ID contains what must restart it (e.g. the
  session number after a new login, the attempt to join a voice channel).
- **Commands** to the gateway (today only op 4 for voice) go through a
  `GatewayHandle`, which the subscription sends to the GUI as soon as it
  starts. Commands are sent only after READY.
- **REST** calls and downloads are iced `Task::perform`s: the result comes
  back as a message (`History`, `Sent`, `PreviewLoaded`, …).
- **`Store`** holds servers, channels, users, messages of the opened channels,
  unread, who is typing and who is in voice channels. It is the same for the
  GUI and the terminal.
- **Permissions** are computed in the `Store` too (`permissions(channel)`,
  `can_send`, `can_connect`), so both interfaces hide the same channels and
  block the same actions. Without the data to compute them (roles or our
  identity not loaded yet) a server channel gets no permission and is not
  listed: missing data never opens anything. `channel_rows` computes our base permissions once
  per server and applies each channel's overwrites, so the list costs little
  even on big servers.

## Interface language

`i18n.rs` keeps the current language in a process-wide atomic (English by
default). Every text the user reads goes through `t("English", "Italiano")`
or `tf!("{n} English", "{n} Italiano", …)`, so both versions sit next to each
other in the code. The language is chosen at startup (`--lang`, then
`RICO_LANG`, then the saved `language` file) and the GUI can switch it
at runtime (the next redraw is in the new language). Logs, internal errors
and comments are English only.

## GUI

iced 0.14 Elm architecture: `Message` (events), `update` (changes the state
and returns a `Task`), `view` (builds the widgets from the state),
`subscription`.

Choices that matter:

- **The root is always a `stack`** (chat, then possibly the picker and the
  full-screen view above). If the structure changed when a layer appears,
  iced would recreate the message list and the scroll position would go back
  to the bottom.
- **The message list is anchored at the bottom** (`anchor_bottom`, set
  *after* `direction`): new messages and old ones loaded while scrolling do not
  make the position jump.
- **Previews** in an in-memory cache (`previews`, at most 400) with a limit on
  concurrent downloads; boxes always have their final size, so the chat does
  not move when an image arrives.
- **Animated GIFs** (`gui/anim.rs`): when downloaded only the first frame is
  decoded; the original file is kept. All frames (scaled down to the display
  size) are decoded on a separate thread when the GIF appears on screen, and
  dropped 15 s after it left the screen, so memory follows what is visible,
  not what was loaded. The widget asks iced for a redraw at the time of the
  next frame, only while visible, rounded up to a common 33 ms grid: several
  GIFs share the same redraws (at most ~30 a second for the whole window) and
  late frames are skipped so the speed stays right.
- **Small images** (server emojis, avatars, server icons, profile banners):
  separate cache (`small_images`, at most 800) with its own download limit;
  avatars and icons are decoded and cut to shape off the interface thread, so
  iced gets ready pixels. Priority: server icons, the open profile, our
  avatar, who is in voice, the authors of the open channel, then emojis.
- **Voice**: `gui::voice::VoiceUi` holds the state (waiting for the server,
  connecting, connected, error), who is speaking, the statistics, the
  end-to-end encryption state, microphone and sound. The connection is a
  subscription that starts when `session_id`, endpoint and token are all there
  (in the demo right away, towards the fake server). Switching channel sends
  only the new channel to Discord: a "leave" first would arrive late and close
  the new connection.

## Voice

The voice connection has three layers, all in `discord/voice`:

1. **Voice gateway** (WebSocket, v8): identify, heartbeat with `seq_ack` (the
   ping comes from here), choice of encryption, secret key, SSRC → user map,
   resume after a drop.
2. **UDP transport**: IP discovery, RTP packets with encrypted audio ("rtpsize"
   modes: header in clear as AAD, 4 byte counter at the end), RTP padding and
   extensions removed after decryption, keepalive.
3. **DAVE** (`voice/dave.rs`): MLS group and per-sender keys, through the
   `davey` library; every Opus frame is end-to-end encrypted before transport.

Audio path:

- incoming: UDP → transport decryption → DAVE → jitter buffer (one per SSRC,
  `audio/jitter.rs`: order by RTP sequence, 100 ms cushion, FEC or
  concealment for lost packets, backlog cut) → Opus decoder → mixer →
  speakers. The mixer is pulled when the speakers' queue drops under 40 ms
  (less than the cushion, or late packets would find their turn gone),
  so the sound card's clock sets the pace; a timer only checks every 10 ms;
- outgoing, only with the microphone on: microphone → voice activity
  detection → Opus encoder → DAVE → transport → UDP.

`src/audio/` is separate from the network: `codec.rs` is the only file that
uses libopus (moving to a Rust codec such as `opus-rs` later only means
changing that file), `device.rs` opens microphone and speakers with cpal
(Windows only, one thread per device, released when the value is dropped),
`dsp.rs` contains bounded queues, resampling, voice activity detection
and the demo's synthetic voice. Protocol details in
[discord-protocol.md](discord-protocol.md).

### Streams

Watching a Go Live stream opens a second voice connection (`run_with_video`)
next to the one of the voice channel. Video path: UDP → transport decryption
→ `video::h264::Depacketizer` (one per SSRC; RTX packets are turned back into
the original ones) → DAVE (`VIDEO`) → `video::decoder` thread (OpenH264 →
RGBA) → `gui::watch::FrameSlot` → `VideoView` widget, which looks for a new
picture about 30 times a second and only redraws (the interface is not
rebuilt). The network side asks again for lost packets (NACK) and for a
keyframe (PLI) when the decoder needs one. In the demo a moving test picture
is drawn instead (no network, no decoder).

Our own stream goes the other way: `capture` (screen or window on the
capture thread, scaled and converted by `video::convert`) → one-place slot →
`video::encoder` thread (OpenH264) → `Outbox` (at most 6 frames) → the
stream connection (`VoiceOptions::golive`): DAVE → `video::h264::packetize`
→ transport → UDP. The program's sound goes into a bounded queue read every
20 ms like a microphone. Dropping `capture::Running` stops everything.

## Demo mode

`--demo` fills the `Store` with fake data (`demo.rs`) and opens no connection
to Discord. The demo's images, GIFs and emojis are generated locally
(`demo://…` addresses), the GIF picker returns fake results, and voice
connects to `discord::voice::mock`, a local voice server speaking the same
protocol as Discord, whose participants are real DAVE members. So the
interface and voice can be tried without an account and without the network.

## Performance

- `compress=zlib-stream` on the gateway: the READY of a user account weighs
  several MB uncompressed.
- The gateway payload is read only once: `op`/`t` right away, `d` stays raw
  (`RawValue`) and is deserialized directly into the final type.
- Messages are in memory only for the opened channels.
- Heavy decoding (GIFs) off the interface thread; limits on frames (300),
  memory (64 MB per GIF) and download sizes.
