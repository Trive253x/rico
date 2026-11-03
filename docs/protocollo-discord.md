# Protocollo Discord: cosa usa ferrocord

Discord non documenta le API per gli account utente: qui c'è ciò che ferrocord
usa e come è stato verificato. Tutti i modelli dati hanno campi opzionali con
`#[serde(default)]`, perché i payload cambiano spesso forma.

## Gateway principale

`wss://gateway.discord.gg/?v=10&encoding=json&compress=zlib-stream`
(`discord/gateway.rs`).

| Op | Direzione | Uso |
|---|---|---|
| 10 Hello | ← | intervallo di heartbeat |
| 2 Identify | → | token, `capabilities: 0`, `properties` con `browser: "ferrocord"` (dichiariamo chi siamo) |
| 6 Resume | → | ripresa della sessione dopo una caduta |
| 1 Heartbeat / 11 ACK | ↔ | connessione viva; senza ACK si riconnette |
| 4 Voice State Update | → | entrare in un canale vocale o uscirne (`channel_id: null`) |
| 7 Reconnect, 9 Invalid Session | ← | riconnessione, eventualmente con nuova sessione |
| 0 Dispatch | ← | eventi, vedi sotto |

Eventi gestiti: `READY`, `RESUMED`, `GUILD_CREATE`, `GUILD_EMOJIS_UPDATE`,
`MESSAGE_CREATE`/`UPDATE`/`DELETE`, `TYPING_START`,
`MESSAGE_REACTION_ADD`/`REMOVE`, `VOICE_STATE_UPDATE`, `VOICE_SERVER_UPDATE`.
Con `capabilities: 0` i server del READY arrivano completi (canali, emoji,
stati vocali).

Codici di chiusura: 4004 token rifiutato (nuovo login), 4010–4014 errore
definitivo, 4007/4009 sessione scaduta (nuova identify), gli altri riconnessione
con backoff esponenziale.

## REST

Base `https://discord.com/api/v10` (`discord/rest.rs`); su 429 si aspetta
`retry_after` e si riprova.

| Endpoint | Uso |
|---|---|
| `GET /users/@me` | verifica del token incollato |
| `GET /channels/{id}/messages?limit=50[&before=]` | cronologia |
| `POST /channels/{id}/messages` | invio (con `nonce`) |
| `GET /gifs/trending-gifs`, `GET /gifs/search?q=` | selettore GIF, con `media_format=tinygif&provider=tenor&locale=it` |

Dal 2026 la ricerca GIF di Discord è fornita da **Klipy**: i risultati sono link
`klipy.com/gifs/…` con file su `static.klipy.com`. Il parametro
`provider=tenor` è ancora accettato. Inviare una GIF significa inviare il suo
link come testo, come fa il client ufficiale.

## Login con QR

`wss://remote-auth-gateway.discord.gg/?v=2` (`discord/remote_auth.rs`): il
client genera una chiave RSA-2048, il server manda un'impronta da mostrare come
QR, l'app sul telefono conferma, il ticket cifrato con RSA-OAEP viene scambiato
con il token su `POST /api/v9/users/@me/remote-auth/login`.

## Media

| Cosa | Da dove |
|---|---|
| Anteprime degli allegati | `media.discordapp.net` con `?width=&height=` |
| Immagini dei link (`embed` `image`) | proxy di Discord (`images-ext-1/2.discordapp.net`) |
| GIF di Tenor | Discord manda solo l'MP4: la GIF si ricava dall'indirizzo, stesso ID con codice `AAAAd` invece di `AAAPo` |
| GIF di Giphy | `…/giphy.mp4` → `…/giphy.gif` |
| GIF di Klipy | miniatura WebP animata dal proxy di Discord, **con `animated=true`** (senza restituisce solo il primo fotogramma) |
| Emoji dei server | `cdn.discordapp.com/emojis/{id}.png?size=48` (`.gif` se animata) |

Gli embed `gifv` arrivano spesso dopo il messaggio, con un `MESSAGE_UPDATE`. Se
il messaggio è solo il link, il testo si nasconde (come su Discord).

## Emoji dei server

Si usano solo nel server a cui appartengono, e solo quelle che Discord
lascerebbe usare: `available`, senza `roles`, e animate solo con Nitro
(`premium_type > 0`). Prima dell'invio `:nome:` diventa `<:nome:id>` (o
`<a:nome:id>`), tranne nel codice tra backtick.

## Voce

Dopo l'op 4 sul gateway principale arrivano `VOICE_STATE_UPDATE` (con la nostra
`session_id`) e `VOICE_SERVER_UPDATE` (`endpoint` e `token`; `endpoint` è
`null` finché Discord non ha scelto il server). Poi (`discord/voice/mod.rs`):

**Gateway vocale**, `wss://{endpoint}/?v=8`:

| Op | Direzione | Uso |
|---|---|---|
| 8 Hello | ← | intervallo di heartbeat |
| 0 Identify | → | `server_id`, `user_id`, `session_id`, `token`, `max_dave_protocol_version: 0` |
| 2 Ready | ← | `ssrc`, `ip`, `port`, cifrature offerte |
| 1 Select Protocol | → | indirizzo e porta pubblici (da IP discovery), cifratura scelta |
| 4 Session Description | ← | `secret_key` (32 byte), `dave_protocol_version` |
| 3 Heartbeat / 6 ACK | ↔ | `{t, seq_ack}`: v8 vuole l'ultimo `seq` ricevuto; da qui il ping |
| 5 Speaking | ↔ | mappa SSRC → utente; in invio, l'indicatore "sta parlando" |
| 7 Resume / 9 Resumed | ↔ | ripresa con la stessa chiave e lo stesso UDP |
| 13 Client Disconnect | ← | un utente è uscito |
| messaggi binari | ↔ | DAVE, non ancora gestito |

Codici di chiusura: 4006 sessione non valida, 4014 disconnesso dal canale,
4016 cifratura non accettata, **4017 DAVE richiesto**: definitivi. 4015 (server
vocale riavviato) e cadute di rete: nuovo tentativo, al massimo 3 di fila.

**UDP**:

- *IP discovery*: richiesta di 74 byte (tipo 1, lunghezza 70, SSRC), risposta
  (tipo 2) con indirizzo e porta pubblici.
- *Audio*: RTP, intestazione di 12 byte (`0x80 0x78`, sequenza, timestamp +960
  per frame da 20 ms, SSRC). Cifratura `aead_aes256_gcm_rtpsize` (preferita) o
  `aead_xchacha20_poly1305_rtpsize`: intestazione in chiaro come AAD,
  contatore di 4 byte big endian in coda al pacchetto, completato con zeri per
  il nonce. Nei pacchetti ricevuti, se c'è un'estensione RTP solo i suoi primi
  4 byte sono in chiaro; i dati dell'estensione sono cifrati prima dell'audio.
- *Keepalive*: 8 byte ogni 5 secondi.
- I frame di silenzio Opus (`F8 FF FE`) non contano come parlato.

**DAVE.** Dal 1° marzo 2026 Discord accetta nelle chiamate solo client con la
crittografia end-to-end DAVE (MLS per le chiavi, cifratura di ogni frame Opus
prima del trasporto). ferrocord dichiara `max_dave_protocol_version: 0`, quindi
un server vero chiude con 4017: per questo con un account vero la voce è
disattivata (`FERROCORD_VOICE=1` per forzarla) e si prova in demo.
