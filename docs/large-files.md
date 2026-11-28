# Files over Discord's limit (analysis)

Analysis for `docs/todo.md` 3.13, written on 27 November 2026 for the
owner. The idea: a file too big for Discord is uploaded to a file-sharing
service and its link is sent instead. Nothing here is implemented; the
choices marked **to decide** are the owner's.

## 0. First: Rico's limit is out of date

Rico refuses more than **10 MiB in total per message**
(`rest::MAX_UPLOAD_BYTES`, also in `gui/compose.rs` and
`gui/messages.rs`). Discord changed its limits since then:

| Account | Discord's limit | Source |
|---|---|---|
| Without Nitro | **20 MB per file** since 13 August 2026 (was 10 MB from September 2024) | Discord's post quoted by Dexerto; a search summary of Discord's own FAQ says 20 MB (the page itself not read) |
| Nitro Basic | 50 MB | same articles |
| Nitro | 500 MB (one headline on the same site says 1 GB: unclear) | same articles |

Discord's help page could not be read from here (it refuses automated
readers), so the numbers come from the press and Discord's post; whether
the limit is per file or per message, and whether server boosts raise it,
was not confirmed.

So today Rico refuses files that Discord would accept: a 15 MB video is
refused for everyone, a 40 MB one for Nitro users. **Fixing this comes
before any external service** (size S):

- the limit per file, from the account's `premium_type` (in the user
  object at login: 0 none, 1 Classic, 2 Nitro, 3 Basic), never more than
  Discord's for that tier;
- if Discord still refuses (HTTP 413 or error 40005), say so clearly:
  then the numbers above are wrong and the limit goes back down;
- check how big uploads must be sent: Discord's own client uploads large
  files first to a temporary address (`POST /channels/{id}/attachments`,
  then the message refers to the uploaded file); Rico today sends them
  inside the message (multipart). To verify with a real 15 MB file;
- the file is still read in memory: at 500 MB that is too much. A bigger
  limit needs the file streamed from disk, not read into a `Vec`.

With 20 MB free, most screenshots, short clips and documents fit, and the
external service is only for what is bigger.

## 1. What the feature must respect (AGENTS.md)

- **A new server contacted**: only after the user's explicit action for
  that file ("Upload to … and send the link?"), with the service named and
  the consequences said (who can read the file, how long it lasts). Listed
  in `docs/privacy-and-security.md` and in the settings; off until chosen.
- **Credentials**: a service with an account keeps its token in the
  system keychain, like Discord's; no plain-text fallback. Never in logs.
- **Network**: HTTPS with certificate checks, only the service's hosts,
  redirects checked, timeouts; the upload goes through a function with
  explicit size and concurrency limits (as `media::fetch` for downloads).
- **No duplicates**: an upload whose result is uncertain is not retried by
  itself; the link is sent once, as a normal message the user sees before
  sending.
- **Interface thread**: reading, encrypting and uploading off it, with
  progress and a Cancel button; memory bounded (the file streamed in
  chunks, not loaded whole).
- **Terms of service**: uploading through a service's documented API is
  allowed use; scraping a website that has no API is not (rules 2 and the
  services' own terms).

## 2. The services

| Service | API | Account | Who can read the file | Size / duration | Rust | Verdict |
|---|---|---|---|---|---|---|
| **SwissTransfer** (Infomaniak) | **none public**: only the website and the apps | no | Infomaniak | large (as advertised; not checked here) | — | **Excluded**: without an API Rico would have to imitate the website, fragile and against the rules |
| **Send** (open-source successor of Firefox Send) | Send v3 protocol, documented by its code; many public servers, and anyone can host one | no | **nobody but who has the link**: encrypted on the PC, the key in the link's `#` part, which the browser never sends to the server; the recipient decrypts in the browser | depends on the server: 21 public ones listed, 50 MiB to 250 GiB, 3 to 30 days, download limits | `ffsend-api` (MIT, used by the `ffsend` tool; its maintenance could not be checked from here) or our own client | **Best fit for privacy**; risk: public servers are run by volunteers, "no promises or warranties"; the owner could host one |
| **MEGA** | yes (official SDK in C++; unofficial Rust ports) | yes (free with storage) | **nobody but who has the link** (MEGA encrypts on the PC, key in the link) | the account's storage; links last until deleted | `megalib` (unofficial, "not affiliated with Mega Limited", license shown as both GPL-2 and MIT: to check), `mega` | Good privacy, but needs a MEGA account and an unofficial library; the token in the keychain |
| **Google Drive** | yes, official (REST v3, resumable uploads) | yes (Google account, OAuth) | **Google**, and anyone with the link | the account's storage | plain HTTPS calls (no SDK needed) | Reliable and familiar; Google can read the file. The `drive.file` scope (only files Rico creates) is non-sensitive: basic verification of Rico's OAuth app by Google, which the owner must register |
| **Litterbox** (catbox.moe) | yes, documented, anonymous | no | the service, and anyone with the link | up to 1 GB, 1 to 72 hours | plain HTTPS | Simple, but no encryption, `.exe`/`.jar`/`.doc*` refused, commercial use needs approval, one person runs it |
| **Catbox** | not documented for this | optional | anyone, "publicly, forever" | — | — | **Excluded**: files stay public for years |
| **Gofile** | its documentation could not be read from here | — | — | — | — | Not evaluated |

### Encryption and who can open the link

Encrypting on the PC before uploading is worth something only if the
recipient can decrypt without Rico: everyone else on Discord uses the
official client and a browser. Send and MEGA do exactly this (the link
opens a page that decrypts in the browser). With Google Drive or
Litterbox, encrypting ourselves would make the link useless to people
without Rico, so the file would travel readable by the service.

## 3. Recommendation

1. **Now, size S: bring Rico's limit to Discord's** (section 0). It solves
   most cases with no new server, no account and no privacy question.
2. **Then, if still wanted, size M: Send**, with the server chosen in the
   settings (empty by default: the feature is off until the user picks a
   server or enters their own). End-to-end encrypted, no account, no token
   to keep, the protocol is open and can be self-hosted. Before starting:
   check `ffsend-api` (maintenance, dependencies, licence, `cargo deny`)
   against writing the few calls Rico needs on top of `reqwest` and the
   AES-GCM and HKDF crates already in the tree.
3. **Optional later: Google Drive** for who prefers it (reliability over
   privacy), after the owner registers Rico's OAuth app with Google.

MEGA stays an alternative to Send if the owner prefers a company to
volunteers' servers; SwissTransfer only if Infomaniak publishes an API.

## 4. How it would work (Send)

1. A file over the limit is dropped or attached: instead of the refusal,
   "*name* (*size*) is bigger than Discord allows. Upload it encrypted to
   *server* and send the link? It will last *N* days, *M* downloads."
   With no server set: the refusal, plus "Settings › Files to set one up".
2. On **Upload**: read and encrypt in chunks on a worker, upload with a
   progress bar in the compose area and a **Cancel**. Bounded memory, one
   upload at a time.
3. When done, the link goes in the text box (not sent by itself): the user
   sees it and presses Send. If the upload fails or is cancelled, nothing
   is sent and nothing is retried by itself.
4. The link sent is a normal message: Discord shows a preview of the Send
   page; the key after `#` is in the message, so whoever can read the
   channel can download the file (said in the prompt).

Tests to add: limits (size, number, duration), Cancel mid-upload, a server
that answers badly or slowly, redirects outside the server, the key never
in logs or errors, the link never sent twice.

## 5. To decide (owner)

- Do step 0 first (limit by Nitro tier)? It changes nothing for privacy.
- Which service for step 2: Send (which server: a public one, or one the
  owner hosts), MEGA, Google Drive, or none for now.
- Is it all right that the decryption key is in the message, readable by
  everyone in the channel (as with any shared link)?
- Default duration and download count, if the service lets us choose.

## Sources

- Discord's limits: Dexerto, "Discord raises free upload limit to 20MB",
  13 August 2026; TechRadar and Dexerto on the 2024 cut to 10 MB; Discord's
  Attachments FAQ (`support.discord.com/hc/articles/25444343291031`, not
  readable from here).
- SwissTransfer: Infomaniak's product and FAQ pages (website and apps
  only); no developer documentation found.
- Send: `github.com/timvisee/send-instances` (21 public servers, sizes and
  durations, its disclaimer); `docs.rs/ffsend-api`.
- MEGA: `docs.rs/crate/megalib` and `lib.rs/crates/megalib` (0.11, March
  2026, unofficial).
- Google Drive: Google's "Choose Google Drive API scopes" (`drive.file`
  non-sensitive, basic verification).
- Catbox and Litterbox: their FAQ, legal and tools pages.
