# Discord protocol: what ferrocord uses

Discord does not document the API for user accounts: this is what ferrocord
uses and how it was verified. Every data model has optional fields with
`#[serde(default)]`, because payloads change shape often.

## Main gateway

`wss://gateway.discord.gg/?v=10&encoding=json&compress=zlib-stream`
(`discord/gateway.rs`).

| Op | Direction | Use |
|---|---|---|
| 10 Hello | ← | heartbeat interval |
| 2 Identify | → | token, `capabilities: 0`, `properties` with `browser: "ferrocord"` (we say who we are) |
| 6 Resume | → | resuming the session after a drop |
| 1 Heartbeat / 11 ACK | ↔ | connection alive; without an ACK it reconnects |
| 4 Voice State Update | → | joining or leaving a voice channel (`channel_id: null`); switching channel sends only the new one |
| 18, 19, 20, 22 Stream Create, Delete, Watch, Set Paused | → | 20 `{stream_key}` to watch a Go Live stream, 19 `{stream_key}` to stop watching or end ours, 18 `{type: "guild", guild_id, channel_id, preferred_region: null}` to start ours, 22 `{stream_key, paused: false}` once Discord created it |
| 37 Guild Subscriptions (bulk) | → | member list of the open server channel: `{"subscriptions": {guild: {"typing": true, "activities": false, "threads": false, "channels": {channel: [[0, 99], …]}}}}`, sent when a server channel is opened (and for the next page, at most 5) |
| 7 Reconnect, 9 Invalid Session | ← | reconnection, possibly with a new session |
| 0 Dispatch | ← | events, see below |

Who is streaming (Go Live) comes from `self_stream` in the voice states.

Handled events: `READY`, `RESUMED`, `GUILD_CREATE`, `GUILD_DELETE`, `GUILD_EMOJIS_UPDATE`,
`CHANNEL_CREATE`/`UPDATE`/`DELETE`, `GUILD_ROLE_CREATE`/`UPDATE`/`DELETE`,
`GUILD_MEMBER_UPDATE`,
`MESSAGE_CREATE`/`UPDATE`/`DELETE`, `TYPING_START`,
`MESSAGE_REACTION_ADD`/`REMOVE`, `VOICE_STATE_UPDATE`, `VOICE_SERVER_UPDATE`,
`STREAM_CREATE`, `STREAM_SERVER_UPDATE`, `STREAM_DELETE`,
`USER_SETTINGS_PROTO_UPDATE` (type 1 only: the server folders),
`GUILD_MEMBER_LIST_UPDATE` (see below).

**Member list.** `GUILD_MEMBER_LIST_UPDATE` carries `guild_id`, the list `id`
(channels with the same visibility share it), `member_count`,
`online_count`, `groups` and `ops`. The list is flat: rows are groups
(`{"group": {"id", "count"}}`, a hoisted role, `online` or `offline`) or
members (`{"member": {user, roles, nick, avatar, presence: {status}}}`).
Ops: `SYNC` (range + items), `INSERT`/`UPDATE` (index + item), `DELETE`
(index), `INVALIDATE` (range). Indexes are checked and the list is kept to
500 rows (`discord/member_list.rs`); a new `id` starts it again.
With `capabilities: 0` the READY servers arrive complete (channels, emojis,
voice states).

Close codes: 4004 token rejected (new login), 4010–4014 final error,
4007/4009 session expired (new identify), the others reconnect with
exponential backoff.

## Permissions

Computed by `permissions.rs` from data already in the READY: the servers'
`roles` (with `permissions` as a decimal string), `owner_id` (or
`properties.owner_id`), each channel's `permission_overwrites` (`type` 0 =
role, 1 = member; also `"role"`/`"member"`), our member in
`merged_members` (roles, `communication_disabled_until`). Order as on
Discord: owner or `ADMINISTRATOR` → everything; otherwise `@everyone` (the
role with the server's ID) plus our roles, then the overwrites of
`@everyone`, of our roles (all denies, then all allows) and of our member;
while timed out only `VIEW_CHANNEL` and `READ_MESSAGE_HISTORY` remain.

What ferrocord does with them:

| Permission | Effect |
|---|---|
| `VIEW_CHANNEL` | channels without it are not listed; a category with no visible channels disappears |
| `READ_MESSAGE_HISTORY` | without it the history is not asked (only new messages are shown) |
| `SEND_MESSAGES` | without it the message field cannot be typed in, and GIF and emoji buttons are hidden |
| `CONNECT` | without it a voice channel shows a padlock and cannot be joined |
| `SPEAK` | without it the microphone cannot be turned on |

A server without roles in the payload is treated as "everything allowed"
(nothing to compute with). The channel, role, member and server events
above keep the data up to date: if the open channel becomes hidden, the
client moves to the first visible one. Discord checks everything again on
its side: this only avoids showing or trying what would be refused.

## REST

Base `https://discord.com/api/v10` (`discord/rest.rs`); on 429 it waits
`retry_after` and tries again.

| Endpoint | Use |
|---|---|
| `GET /users/@me` | checking a pasted token |
| `GET /channels/{id}/messages?limit=50[&before=]` | history |
| `GET /users/{id}/profile?with_mutual_guilds=false&with_mutual_friends=false[&guild_id=]` | profile card, only when it is opened; with `guild_id` it also has `guild_member` (nickname, server avatar and banner, roles, `joined_at`) and `guild_member_profile` (server bio, pronouns) |
| `GET /streams/guild:{guild}:{channel}:{user}/preview` | Go Live preview, only when the user opens it: `{"url"}` pointing to `cdn.discordapp.com/streams/…` (about 512×288), 404 while there is none |
| `POST /channels/{id}/messages` | sending (with a `nonce`) |
| `GET /users/@me/settings-proto/1` | account settings after `READY`: the server folders |
| `PATCH /users/@me/settings-proto/1` | saving the folders, only from the editor's "Save" (see below) |
| `PATCH /guilds/{g}/members/{u}` | moderation from the user card: `{"mute"}`, `{"deaf"}`, `{"channel_id"}` (move, `null` = disconnect), `{"communication_disabled_until"}` (timeout up to 28 days, `null` = remove) |
| `DELETE /guilds/{g}/members/{u}` | kick (after the confirmation in the card) |
| `PUT /guilds/{g}/bans/{u}` | ban with `{"delete_message_seconds": 0}` (after the confirmation) |
| `GET /gifs/trending-gifs`, `GET /gifs/search?q=` | GIF picker, with `media_format=tinygif&provider=tenor&locale=<interface language>` |

Since 2026 Discord's GIF search is provided by **Klipy**: results are
`klipy.com/gifs/…` links with files on `static.klipy.com`. The
`provider=tenor` parameter is still accepted. Sending a GIF means sending its
link as text, as the official client does.

## QR login

`wss://remote-auth-gateway.discord.gg/?v=2` (`discord/remote_auth.rs`): the
client generates an RSA-2048 key, the server sends a fingerprint to show as a
QR code, the phone app confirms, and the ticket encrypted with RSA-OAEP is
exchanged for the token on `POST /api/v9/users/@me/remote-auth/login`.

## Media

| What | From where |
|---|---|
| Attachment previews | `media.discordapp.net` with `?width=&height=` |
| Link images (`image` embed) | Discord's proxy (`images-ext-1/2.discordapp.net`) |
| Tenor and Giphy GIFs | the MP4 clip from Discord's proxy (`video.proxy_url` of the `gifv` embed, on `images-ext-1.discordapp.net`), decoded on the PC; still picture (`thumbnail.proxy_url`) while it loads or if it fails. The proxy does not turn the clip into an animated image: `?format=webp` gives one frame, `?format=gif` is refused (415) |
| Klipy GIFs | animated WebP thumbnail from Discord's proxy, **with `animated=true`** (without it only the first frame comes back) |
| Server emojis | `cdn.discordapp.com/emojis/{id}.png?size=48` (`.gif` if animated) |
| Avatars | `cdn.discordapp.com/avatars/{user}/{hash}.png?size=64` (256 in the profile card); server profile: `guilds/{guild}/users/{user}/avatars/{hash}.png` |
| Server icons | `cdn.discordapp.com/icons/{guild}/{hash}.png?size=128` |
| Group conversation pictures | `cdn.discordapp.com/channel-icons/{channel}/{hash}.png?size=64` (one-to-one conversations show the other person's avatar) |
| Profile banners | `banners/{user}/{hash}.png?size=1024`; server profile: `guilds/{guild}/users/{user}/banners/{hash}.png` |

The CDN answers **HTTP 400** to a `size` that is not a power of two between
16 and 4096 (e.g. 168): `media::cdn_png` always rounds up to the next one and
the picture is scaled down after decoding.

`gifv` embeds often arrive after the message, with a `MESSAGE_UPDATE`. If the
message is only the link, the text is hidden (as on Discord).

Avatars, icons and banners are always asked as static PNGs (animated ones,
`a_…`, show their first frame) and only if the hash is valid (32 hex digits,
optional `a_`) and the IDs are numeric: anything else from the network never
ends up in an address. The picture is decoded with size limits and cut to a
circle (avatars) or a rounded square (server icons) before it reaches the
interface. Without an avatar the initials are shown.

Names and server avatars: the author of gateway messages comes with
`member` (nickname, server avatar, roles); our own member in each server is
in the READY's `merged_members`; the others are learned from opened profiles.
History downloaded over REST does not have `member`, so a nickname may show
up only after the first live message or after opening the profile.

## Server emojis

They are used only in the server they belong to, and only those Discord would
allow: `available`, without `roles`, and animated ones only with Nitro
(`premium_type > 0`). Before sending, `:name:` becomes `<:name:id>` (or
`<a:name:id>`), except in code between backticks.

## Voice

After op 4 on the main gateway, `VOICE_STATE_UPDATE` (with our `session_id`)
and `VOICE_SERVER_UPDATE` (`endpoint` and `token`; `endpoint` is `null` until
Discord has chosen the server) arrive. Discord does not send our state again
if we ask for the channel we are already in: the last `session_id` is reused.
A "left" state that arrives before Discord confirmed the current join belongs
to the previous channel and is ignored. Then (`discord/voice/mod.rs`):

**Voice gateway**, `wss://{endpoint}/?v=8`:

| Op | Direction | Use |
|---|---|---|
| 8 Hello | ← | heartbeat interval |
| 0 Identify | → | `server_id`, `user_id`, `session_id`, `token`, `max_dave_protocol_version: 1` |
| 2 Ready | ← | `ssrc`, `ip`, `port`, offered encryption modes |
| 1 Select Protocol | → | public address and port (from IP discovery), chosen mode |
| 4 Session Description | ← | `secret_key` (32 bytes), `dave_protocol_version` |
| 3 Heartbeat / 6 ACK | ↔ | `{t, seq_ack}`: v8 wants the last `seq` received; the ping comes from here |
| 5 Speaking | ↔ | SSRC → user map; when sending, the "speaking" indicator |
| 7 Resume / 9 Resumed | ↔ | resume with the same key and the same UDP socket |
| 11 Clients Connect | ← | users in the call (DAVE accepts only them in the MLS group) |
| 13 Client Disconnect | ← | a user left |
| 21–31 | ↔ | DAVE, see below |

Close codes: 4006 invalid session, 4014 disconnected from the channel, 4016
encryption not accepted, **4017 DAVE required**: final. 4015 (voice server
restarted) and network drops: new attempt, at most 3 in a row.

**UDP**:

- *IP discovery*: 74 byte request (type 1, length 70, SSRC), response
  (type 2) with the public address and port.
- *Audio*: RTP, 12 byte header (`0x80 0x78`, sequence, timestamp +960 per
  20 ms frame, SSRC). Encryption `aead_aes256_gcm_rtpsize` (preferred) or
  `aead_xchacha20_poly1305_rtpsize`: header in clear as AAD, 4 byte big endian
  counter at the end of the packet, padded with zeros for the nonce. In
  received packets, if there is an RTP extension only its first 4 bytes are in
  clear; the extension data is encrypted before the audio. RTP padding (P bit)
  is inside the encrypted part too and is removed after decryption: the
  official mobile client uses it.
- *Keepalive*: 8 bytes every 5 seconds.
- Opus silence frames (`F8 FF FE`) do not count as speech and are not
  end-to-end encrypted; they still go to the jitter buffer, where they mark
  the end of a sentence (playback stops cleanly instead of concealing).
- Received audio is played by RTP sequence number, not arrival order: a
  missing packet is rebuilt from the FEC data of the next one, or concealed
  (Opus PLC) for up to 60 ms; late packets are dropped.

**DAVE** (`discord/voice/dave.rs`, on top of the `davey` library). Since
1 March 2026 Discord only accepts clients with DAVE end-to-end encryption in
calls: MLS (RFC 9420, ciphersuite 2, P-256) for the keys, encryption of every
Opus frame before transport. Specification: <https://daveprotocol.com>.

| Op | Format | Direction | Use |
|---|---|---|---|
| 21 Prepare Transition | JSON | ← | `transition_id`, `protocol_version`; answered with 23 |
| 22 Execute Transition | JSON | ← | switch to the new keys |
| 23 Ready for Transition | JSON | → | ready for the transition |
| 24 Prepare Epoch | JSON | ← | `epoch: 1` = new group: new key package |
| 25 External Sender | binary | ← | the server's MLS identity (signs the proposals) |
| 26 Key Package | binary | → | our MLS key package (single use) |
| 27 Proposals | binary | ← | add/remove proposals to sign with a commit |
| 28 Commit Welcome | binary | → | our commit, plus the welcome for whoever joins |
| 29 Announce Commit | binary | ← | `transition_id` + commit to apply |
| 30 Welcome | binary | ← | `transition_id` + welcome to join the group |
| 31 Invalid Commit Welcome | JSON | → | invalid commit/welcome: start again with a new key package |

Binary messages from the server are `[seq u16 BE][opcode][payload]`, those
from the client `[opcode][payload]`. ferrocord's rules:

- `dave_protocol_version: 0` in the Session Description, or a transition to
  version 0: the call is closed (no voice without end-to-end encryption);
- no passthrough: frames that do not decrypt with DAVE are dropped;
- our audio starts only when the group key is ready;
- welcome and commit are accepted only if every group member is among the
  users the server announced (op 11/13);
- the privacy code (30 digits) is shown in the voice panel; per-user
  verification codes (45 digits) are available but not shown yet.

Verified with Discord's servers on 2026-11-07 (owner's account, microphone
off): DAVE v1 accepted, group formed with the official mobile client, same
privacy code on both sides, audio received and played.

## Watching a stream (Go Live)

The stream key is `guild:{guild}:{channel}:{user}`. Discord lets you watch only
from inside the stream's voice channel, so ferrocord joins it first (op 4,
microphone off), then (`gui/watch.rs`):

1. op 20 `{stream_key}` on the main gateway;
2. `STREAM_CREATE` `{stream_key, rtc_server_id, …}` and
   `STREAM_SERVER_UPDATE` `{stream_key, endpoint, token}` (a new one restarts
   the connection); `STREAM_DELETE` `{stream_key, reason}` ends it
   (`stream_full`, `unauthorized`, or the stream stopped);
3. a **second voice connection**, the same protocol as voice above, with
   these differences:
   - Identify: `server_id` is the `rtc_server_id`, plus `video: true` and
     `streams: [{type: "video", rid: "100", quality: 100}]`;
   - DAVE: the group's channel ID is `rtc_server_id − 1` (the stream
     connection has its own MLS group, with the streamer);
   - Select Protocol: `codecs` with Opus (payload type 120) and H.264 (103,
     retransmissions on 104), `decode: true`, `encode: false`;
   - after op 4 the client sends op 12 (Video: `audio_ssrc`, `video_ssrc: 0`,
     no streams of its own) and op 15 (Media Sink Wants `{"any": 100}`); the
     server's op 12 maps `user_id` to `audio_ssrc`, `video_ssrc` and
     `streams[].{ssrc, rtx_ssrc}`.
4. op 19 to stop watching closes only the second connection: you stay in the
   voice channel.

**Video over RTP** (`video/h264.rs`):

- H.264 per RFC 6184: single NAL units, STAP-A (type 24) and FU-A (type 28).
  Packets are put back in order by sequence number (64-bit, wraps handled);
  a frame ends at the marker bit or when the timestamp changes; padding-only
  packets are skipped.
- After transport decryption each frame (Annex B, 4-byte start codes) is
  decrypted with DAVE as `VIDEO` media.
- Lost packets: RTCP NACK (`0x81`, type 205, PID + BLP) at most 3 times per
  packet, every 100 ms; retransmissions come on the RTX SSRC (RFC 4588: the
  original sequence number, then the original payload). A frame still
  missing after 4 later complete frames is given up.
- Broken picture: RTCP PLI (`0x81`, type 206) at most every 2 s, until a
  keyframe (IDR, NAL 5, with SPS/PPS 7/8) arrives.
- RTCP is encrypted like RTP in the rtpsize modes: the first 8 bytes in clear
  as AAD, the rest encrypted, 4-byte counter at the end.

## Streaming our screen (Go Live)

`gui/golive.rs`, `capture/`, `video/encoder.rs`, `discord/voice/mod.rs`:

1. op 18 `{type: "guild", guild_id, channel_id, preferred_region: null}` (we
   must be in the voice channel); Discord answers with `STREAM_CREATE` for
   our key `guild:{guild}:{channel}:{us}`, then we send op 22
   `{stream_key, paused: false}`; `STREAM_SERVER_UPDATE` gives the server;
2. a stream connection like the one for watching, with these differences:
   - Identify: `video: true`, `streams: [{type: "screen", rid: "100",
     quality: 100}]`; Ready answers with `streams[].{ssrc, rtx_ssrc}` for our
     video (if missing, the SSRC after ours and the one after that);
   - Select Protocol: H.264 with `encode: true`;
   - after op 4: op 12 `{audio_ssrc, video_ssrc, rtx_ssrc, streams: [{type:
     "screen", rid: "100", ssrc, rtx_ssrc, active: true, quality: 100,
     max_bitrate, max_framerate, max_resolution: {type: "fixed", width,
     height}}]}`, then op 5 speaking on the audio SSRC;
3. each encoded frame (Annex B) is encrypted with DAVE as `VIDEO`/`H264`
   (NAL headers in clear, as the protocol wants), split into packets of at
   most 1100 bytes (single NAL units, FU-A for larger ones), payload type
   103, marker on the last packet, 90 kHz timestamp. Each packet has the
   RTP extension bit, the extension's profile and length (`BE DE 00 01`) in
   clear and one element encrypted with the payload: playout delay (id 5)
   set to zero;
4. viewers' RTCP (decrypted like RTP, compound packets): PLI (206/1) and FIR
   (206/4) ask the encoder for a keyframe; NACK (205/1) sends the lost
   packets again (the last 1024 are kept) on the RTX SSRC with payload type
   104 (original sequence number first, RFC 4588). A sender report (200)
   for the video and the sound goes out every second;
5. the sound (48 kHz stereo) is Opus, DAVE, then like the voice on the
   audio SSRC; silence sends only a few silence frames;
6. op 19 `{stream_key}` ends the stream; `STREAM_DELETE` for our key also
   ends it on our side.

The encoder is OpenH264, Baseline profile, screen-content mode, bit-rate
control (2.5 Mbit/s at 720p 30), a keyframe at least every 10 s; when the
screen does not change the last picture is encoded again once a second.

Verified with the demo's fake voice server (real DAVE participants decrypt
the video and the sound, ask for a keyframe; the frames decode); on
Discord's servers still to try.

Discord's streams are H.264 High profile (100), I and P frames only. OpenH264
up to 2.6 loses picture buffers when it is flushed in single-thread mode
(cisco/openh264 PR #4037): with this profile decoding stops after a few
frames. ferrocord therefore never flushes the decoder while watching.

Verified with Discord's servers on 2026-11-08 (owner's account, microphone
off): 1280×720 stream from the official client, about 30 pictures per
second, about 1 in 160 frames lost.

## Server folders

The folders are in the account settings, a protobuf (`PreloadedUserSettings`)
carried as base64 in JSON (`discord/folders.rs`, a minimal decoder with
limits: at most 4 MB, 1000 folders, 1000 servers per folder, names cut to 64
characters):

- field 1 `versions` → 3 `data_version`;
- field 14 `guild_folders` → `GuildFolders { 1 repeated GuildFolder folders,
  2 repeated fixed64 guild_positions (deprecated) }`;
- `GuildFolder { 1 repeated fixed64 guild_ids, 2 Int64Value id,
  3 StringValue name, 4 UInt64Value color }`. An entry without `id` is a
  single server outside folders (it keeps the order).

Read with `GET /users/@me/settings-proto/1` (`{"settings": base64}`) after
`READY`; `USER_SETTINGS_PROTO_UPDATE` `{settings: {type, proto}, partial}`
keeps them in sync (with `partial` only the changed top-level fields are
there: the folders are replaced only if field 14 is present).

Saving: `PATCH /users/@me/settings-proto/1` with `{"settings": base64,
"required_data_version": n}`. The PATCH replaces each top-level field it
carries, so the body is a `PreloadedUserSettings` with field 14 only, whole
(fields of a folder ferrocord does not know are sent back as received). If
the settings changed meanwhile Discord answers `out_of_date: true` and keeps
its version: the editor reloads it. The answer's `settings` are the settings
after the change. The endpoint is rate limited: a save happens only on
"Save", never on each edit.

Servers not yet in the settings (just joined) are shown on top, as Discord
does; ids of servers we are no longer in are kept (they may be only
unavailable) but not shown.
