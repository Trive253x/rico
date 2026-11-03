# Architettura

ferrocord è un'unica libreria (`src/lib.rs`) con due programmi sopra: la GUI
(`src/main.rs`) e il terminale (`src/bin/ferrocord-tui.rs`). Il codice di rete
e i dati non sanno quale interfaccia li sta usando.

## Moduli

```
src/
├── main.rs                 programma `ferrocord` (GUI)
├── bin/ferrocord-tui.rs    programma `ferrocord-tui` (terminale)
├── lib.rs                  opzioni da riga di comando, avvio, logging
├── config.rs               percorsi, salvataggio del token, file di log
├── store.rs                dati della sessione, condivisi dalle due interfacce
├── text.rs                 tag di Discord (<@id>, <#id>, <:emoji:id>, <t:…>)
│                           resi leggibili; `:nome:` → emoji del server all'invio
├── media.rs                download di immagini, GIF ed emoji: server ammessi,
│                           limiti, decodifica delle animazioni, salvataggio file
├── demo.rs                 dati finti per --demo
├── discord/
│   ├── gateway.rs          WebSocket principale: identify/resume, heartbeat,
│   │                       eventi, comandi (op 4 per la voce)
│   ├── zlib.rs             decompressione zlib-stream del gateway
│   ├── rest.rs             API REST: profilo, cronologia, invio, ricerca GIF
│   ├── remote_auth.rs      login con QR (RSA-OAEP + ticket)
│   ├── models.rs           modelli dati, tolleranti ai campi mancanti
│   └── voice/
│       ├── mod.rs          connessione al server vocale (gateway vocale v8,
│       │                   UDP, ping, chi parla, riconnessione)
│       ├── crypto.rs       cifratura dei pacchetti audio (AES-GCM, XChaCha20)
│       ├── rtp.rs          intestazioni RTP e IP discovery
│       └── mock.rs         server vocale finto in locale (demo e test)
├── gui/
│   ├── mod.rs              stato, messaggi, update e view della GUI
│   ├── style.rs            tema One Dark e stili dei widget
│   ├── anim.rs             widget per le GIF animate
│   ├── picker.rs           selettore di GIF ed emoji
│   └── voice.rs            stato della voce e pannello "Connesso alla voce"
└── tui/                    interfaccia da terminale (ratatui)
```

## Flusso dei dati

```
             eventi                       messaggi iced
Discord ──► gateway.rs ──► canale mpsc ──► Gui::update ──► Store ──► view
   ▲                                          │
   └──────── rest.rs / comandi gateway ◄──────┘  (Task asincroni)
```

- **Gateway, login QR e voce** girano come task tokio e mandano i loro eventi
  su un canale (`UnboundedSender<E>` con `E: From<Evento>`). Ogni interfaccia
  li converte nei propri messaggi.
- Nella **GUI** questi task sono *sottoscrizioni* di iced
  (`Subscription::run_with_id`): iced le avvia e le ferma da solo quando cambia
  lo stato. L'ID della sottoscrizione contiene ciò che deve farla ripartire
  (es. il numero di sessione dopo un nuovo login, il tentativo di ingresso in
  un canale vocale).
- I **comandi** verso il gateway (oggi solo l'op 4 della voce) passano da un
  `GatewayHandle`, che la sottoscrizione manda alla GUI appena parte. I comandi
  vengono inviati solo dopo il READY.
- Le chiamate **REST** e i download sono `Task::perform` di iced: il risultato
  torna come messaggio (`History`, `Sent`, `PreviewLoaded`, …).
- **`Store`** tiene server, canali, utenti, messaggi dei canali aperti, non
  letti, chi sta scrivendo e chi è nei canali vocali. È lo stesso per GUI e
  terminale.

## GUI

Architettura Elm di iced 0.13: `Message` (eventi), `update` (cambia lo stato e
restituisce `Task`), `view` (costruisce i widget dallo stato), `subscription`.

Scelte che contano:

- **La radice è sempre uno `stack`** (chat, poi eventualmente selettore e vista
  a schermo intero sopra). Se la struttura cambiasse quando un livello compare,
  iced ricreerebbe la lista dei messaggi e la posizione di scorrimento
  tornerebbe in fondo.
- **La lista dei messaggi è ancorata in basso** (`anchor_bottom`, impostato
  *dopo* `direction`): i messaggi nuovi e quelli vecchi caricati scorrendo non
  fanno saltare la posizione.
- **Anteprime** in una cache in memoria (`previews`, al massimo 400) con un
  limite di download contemporanei; i riquadri hanno sempre la misura finale,
  così la chat non si sposta quando un'immagine arriva.
- **GIF animate**: i fotogrammi vengono decodificati in un thread a parte e
  ridotti alla misura di visualizzazione; il widget `anim::AnimatedImage` chiede
  a iced un ridisegno all'istante del fotogramma successivo, e solo finché è
  visibile.
- **Emoji dei server**: cache separata (`emoji_images`), immagini a 48 px dal
  CDN di Discord.
- **Voce**: `gui::voice::VoiceUi` tiene lo stato (in attesa del server,
  connessione, connesso, errore), chi parla e le statistiche; la connessione è
  una sottoscrizione che parte quando ci sono `session_id`, endpoint e token (in
  demo subito, verso il server finto).

## Voce

La connessione vocale ha due livelli, entrambi in `discord/voice`:

1. **Gateway vocale** (WebSocket, v8): identify, heartbeat con `seq_ack` (da qui
   il ping), scelta della cifratura, chiave segreta, mappa SSRC → utente,
   resume dopo una caduta.
2. **Trasporto UDP**: IP discovery, pacchetti RTP con l'audio cifrato
   (modalità "rtpsize": intestazione in chiaro come AAD, contatore di 4 byte in
   coda), keepalive.

Il contenuto dei pacchetti è audio Opus: oggi viene solo contato (chi parla,
pacchetti ricevuti e inviati). Mancano la decodifica e codifica Opus, microfono
e altoparlanti, e la crittografia end-to-end DAVE, che va inserita tra Opus e
la cifratura del trasporto. Dettagli del protocollo in
[protocollo-discord.md](protocollo-discord.md).

## Modalità demo

`--demo` riempie lo `Store` con dati finti (`demo.rs`) e non apre connessioni a
Discord. Le immagini, le GIF e le emoji della demo sono generate in locale
(indirizzi `demo://…`), il selettore GIF restituisce risultati finti, e la voce
si collega a `discord::voice::mock`, un server vocale in locale che parla lo
stesso protocollo di Discord. Così l'interfaccia e la voce si provano senza
account e senza rete.

## Prestazioni

- `compress=zlib-stream` sul gateway: il READY di un account utente pesa
  diversi MB non compressi.
- Il payload del gateway viene letto una volta sola: `op`/`t` subito, `d` resta
  grezzo (`RawValue`) e viene deserializzato direttamente nel tipo finale.
- I messaggi sono in memoria solo per i canali aperti.
- Decodifiche pesanti (GIF) fuori dal thread dell'interfaccia; limiti di
  fotogrammi (300), memoria (64 MB per GIF) e dimensioni dei download.
