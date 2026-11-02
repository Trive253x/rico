# ferrocord

Client Discord alternativo per il terminale, scritto in Rust: nativo, leggero,
senza Electron né Chromium.

> **Attenzione.** È un client **non ufficiale**. I Termini di Servizio di Discord
> vietano l'accesso al servizio con client di terze parti e l'automazione degli
> account utente: usarlo può portare alla chiusura dell'account. Per sviluppo e
> test usa un **account secondario**, e non aggiungere mai automazioni
> (invii automatici, scraping, self-bot).

## Stato attuale

| Funzione | Stato |
|---|---|
| Login con QR code (app Discord sul telefono) | ✅ |
| Login con token incollato | ✅ |
| Token salvato localmente, leggibile solo dall'utente | ✅ |
| Gateway: identify, heartbeat, resume, riconnessione con backoff, `zlib-stream` | ✅ |
| Server, categorie, canali testuali, messaggi diretti | ✅ |
| Cronologia con caricamento dei messaggi precedenti | ✅ |
| Messaggi in tempo reale, modifiche, eliminazioni, non letti | ✅ |
| Invio messaggi | ✅ |
| Menzioni, canali, emoji personalizzate, timestamp resi leggibili | ✅ |
| Modalità demo senza rete (`--demo`) | ✅ |
| Permessi per canale (oggi si vedono anche canali non accessibili) | ⏳ |
| Thread, forum, reazioni, risposte, upload | ⏳ |
| Immagini (protocolli Kitty/Sixel) | ⏳ |
| Voce | ⏳ |

## Compilare

Serve Rust stabile ≥ 1.82 ([rustup.rs](https://rustup.rs)). Su Windows servono
anche i Build Tools di Visual Studio (li chiede già l'installer di Rust).

```sh
cargo run --release            # client vero
cargo run --release -- --demo  # interfaccia con dati finti, nessuna connessione
cargo test                     # test di protocollo, formattazione e interfaccia
```

Il binario finisce in `target/release/ferrocord` (`.exe` su Windows).

## Uso

All'avvio compare la schermata di accesso:

- **QR code** (consigliato): apri l'app Discord sul telefono → Impostazioni →
  *Scansiona codice QR* e conferma. La password non passa mai dal client.
- **Token**: premi `Tab`, incolla il token e premi `Invio`. Il token viene
  verificato prima di essere salvato.

Il token viene salvato in:

| Sistema | Percorso |
|---|---|
| Linux | `~/.config/ferrocord/token` (permessi 600) |
| macOS | `~/Library/Application Support/dev.ferrocord.ferrocord/token` |
| Windows | `%APPDATA%\ferrocord\ferrocord\config\token` |

`ferrocord --logout` lo cancella. La variabile `FERROCORD_TOKEN` ha la precedenza
sul file salvato.

### Tasti

| Tasto | Azione |
|---|---|
| `Tab` / `Shift+Tab` | pannello successivo / precedente |
| `↑` `↓` (oppure `k` `j`) | muoversi tra server, canali, messaggi |
| `Invio` | apri server o canale, invia messaggio |
| `Esc` / `←` | torna al pannello precedente |
| `PgSu` / `PgGiù` | scorri i messaggi (in cima carica i precedenti) |
| `Fine` | torna agli ultimi messaggi |
| `Ctrl+W` / `Ctrl+U` | cancella parola / riga nel campo di testo |
| `Ctrl+X` | esci dall'account |
| `Ctrl+C` | chiudi il programma |

Il log va in un file (lo standard output è occupato dall'interfaccia):
`~/.local/share/ferrocord/ferrocord.log` su Linux. Il livello si regola con
`FERROCORD_LOG=debug`. Token e payload non vengono mai scritti nel log.

## Architettura

```
src/
├── main.rs           avvio, terminale, loop degli eventi
├── app.rs            stato e gestione degli eventi (tastiera + rete)
├── config.rs         percorsi, salvataggio del token, logging su file
├── input.rs          campo di testo con cursore (Unicode, AltGr)
├── demo.rs           dati finti per --demo
├── discord/
│   ├── gateway.rs    WebSocket: identify/resume, heartbeat, eventi
│   ├── zlib.rs       decompressione zlib-stream del gateway
│   ├── rest.rs       API REST: cronologia, invio, profilo, rate limit
│   ├── remote_auth.rs login con QR (RSA-OAEP + ticket)
│   └── models.rs     modelli dati tolleranti ai campi mancanti
└── ui/
    ├── mod.rs        layout principale, liste, chat, barra di stato
    ├── format.rs     raggruppamento messaggi, menzioni, a capo
    └── login.rs      schermata di accesso
```

Tutte le operazioni di rete girano in task tokio separati e comunicano con
l'interfaccia tramite un unico canale di `AppEvent`. Il loop principale
ridisegna solo quando arriva un evento: a riposo il processo non si sveglia
(le animazioni hanno un timer solo mentre sono visibili). Le raffiche di eventi
vengono applicate tutte prima di un singolo ridisegno.

Scelte che contano per le prestazioni:

- `compress=zlib-stream` sul gateway: il READY di un account utente pesa
  diversi MB non compressi;
- il payload del gateway viene letto una volta sola: `op`/`t` subito, il campo
  `d` resta grezzo (`RawValue`) e viene deserializzato direttamente nel tipo
  finale, senza passare da un albero JSON intermedio;
- i messaggi vengono tenuti in memoria solo per i canali aperti.

## Prossimi passi consigliati

1. Calcolo dei permessi (ruoli + override del canale) per nascondere i canali
   non leggibili.
2. Risposte, reazioni e thread.
3. Cache su disco di server e canali per un avvio istantaneo.
4. Interfaccia grafica nativa (`iced`, `egui` o `gpui`) riusando `discord/` e `app.rs`.

## Licenza

MIT
