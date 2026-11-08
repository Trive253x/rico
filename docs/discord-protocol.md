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
| 7 Reconnect, 9 Invalid Session | ← | reconnection, possibly with a new session |
| 0 Dispatch | ← | events, see below |

Handled events: `READY`, `RESUMED`, `GUILD_CREATE`, `GUILD_EMOJIS_UPDATE`,
`MESSAGE_CREATE`/`UPDATE`/`DELETE`, `TYPING_START`,
`MESSAGE_REACTION_ADD`/`REMOVE`, `VOICE_STATE_UPDATE`, `VOICE_SERVER_UPDATE`.
With `capabilities: 0` the READY servers arrive complete (channels, emojis,
voice states).

Close codes: 4004 token rejected (new login), 4010–4014 final error,
4007/4009 session expired (new identify), the others reconnect with
exponential backoff.

## REST

Base `https://discord.com/api/v10` (`discord/rest.rs`); on 429 it waits
`retry_after` and tries again.

| Endpoint | Use |
|---|---|
| `GET /users/@me` | checking a pasted token |
| `GET /channels/{id}/messages?limit=50[&before=]` | history |
| `GET /users/{id}/profile?with_mutual_guilds=false&with_mutual_friends=false[&guild_id=]` | profile card, only when it is opened; with `guild_id` it also has `guild_member` (nickname, server avatar and banner, roles, `joined_at`) and `guild_member_profile` (server bio, pronouns) |
| `POST /channels/{id}/messages` | sending (with a `nonce`) |
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
| Tenor GIFs | Discord only sends the MP4: the GIF is derived from the address, same ID with code `AAAAd` instead of `AAAPo` |
| Giphy GIFs | `…/giphy.mp4` → `…/giphy.gif` |
| Klipy GIFs | animated WebP thumbnail from Discord's proxy, **with `animated=true`** (without it only the first frame comes back) |
| Server emojis | `cdn.discordapp.com/emojis/{id}.png?size=48` (`.gif` if animated) |
| Avatars | `cdn.discordapp.com/avatars/{user}/{hash}.png?size=64` (256 in the profile card); server profile: `guilds/{guild}/users/{user}/avatars/{hash}.png` |
| Server icons | `cdn.discordapp.com/icons/{guild}/{hash}.png?size=128` |
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
- Opus silence frames (`F8 FF FE`) do not count as speech.

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
