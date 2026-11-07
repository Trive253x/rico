# ferrocord

Client Discord alternativo scritto in Rust: nativo, leggero, senza Electron né
Chromium. Due interfacce sugli stessi dati:

- **`ferrocord`**: interfaccia grafica (iced), quella installata dall'installer;
- **`ferrocord-tui`**: interfaccia da terminale (ratatui).

> **Attenzione.** È un client **non ufficiale**. I Termini di Servizio di Discord
> vietano l'accesso al servizio con client di terze parti e l'automazione degli
> account utente: usarlo può portare alla chiusura dell'account. Per sviluppo e
> test usa la **modalità demo** o un **account secondario**, e non aggiungere mai
> automazioni (invii automatici, scraping, self-bot).

## Documentazione

| Documento | Contenuto |
|---|---|
| [docs/architettura.md](docs/architettura.md) | moduli, flusso dei dati, scelte di progetto |
| [docs/protocollo-discord.md](docs/protocollo-discord.md) | gateway, REST, login QR, media, GIF, voce: cosa usa ferrocord e come |
| [docs/sviluppo.md](docs/sviluppo.md) | compilare, provare, test, CI, branch e convenzioni |
| [docs/privacy-e-sicurezza.md](docs/privacy-e-sicurezza.md) | token, log, server contattati, controlli automatici |
| [AGENTS.md](AGENTS.md) | istruzioni per gli assistenti AI che lavorano sul codice |

## Stato attuale

| Funzione | Stato |
|---|---|
| Login con QR code (app Discord sul telefono) o con token | ✅ |
| Token salvato localmente, leggibile solo dall'utente | ✅ |
| Gateway: identify, heartbeat, resume, riconnessione con backoff, `zlib-stream` | ✅ |
| Server, categorie, canali testuali, messaggi diretti, non letti | ✅ |
| Cronologia con caricamento dei messaggi precedenti | ✅ |
| Messaggi in tempo reale, modifiche, eliminazioni, invio | ✅ |
| Menzioni, canali, timestamp resi leggibili; risposte, reazioni, "sta scrivendo…" | ✅ |
| Anteprime delle immagini allegate, vista a schermo intero con zoom, salvataggio | ✅ |
| GIF animate: link Tenor, Giphy e Klipy, GIF allegate | ✅ |
| Selettore di GIF (ricerca di Discord) ed emoji (Unicode e del server) | ✅ |
| Emoji dei server nei messaggi e nelle reazioni, usabili solo nel loro server | ✅ |
| Chat vocale: chi è nei canali vocali, connessione al server vocale, audio cifrato | ✅ |
| Chat vocale: audio Opus, microfono e altoparlanti, crittografia end-to-end DAVE | 🚧 provata in demo, non ancora con Discord |
| Permessi per canale (oggi si vedono anche canali non accessibili) | ⏳ |
| Rispondere, reagire, caricare file dall'interfaccia | ⏳ |
| Avatar veri, anteprime dei link normali, messaggi "card" dei bot | ⏳ |
| Modalità demo senza account (`--demo`) | ✅ |
| Installer Windows a ogni push, controlli di sicurezza automatici | ✅ |

**Chat vocale.** L'audio è codificato con Opus (libopus) e cifrato end-to-end
con DAVE, che Discord richiede dal 1° marzo 2026: se il server non attiva DAVE
la chiamata viene chiusa, mai proseguita senza cifratura end-to-end. Il
microfono si apre solo con il pulsante "Microphone" e si rilascia spegnendolo,
silenziando l'audio o uscendo. Microfono e altoparlanti per ora solo su
Windows. Tutto è provato con `--demo` (server vocale finto in locale, con
partecipanti DAVE veri); con i server di Discord non è ancora stato provato.

## Compilare

Serve Rust stabile ≥ 1.85 ([rustup.rs](https://rustup.rs)). Su Windows servono
anche i Build Tools di Visual Studio (li chiede già l'installer di Rust), con
il componente CMake: serve a compilare libopus.

```sh
cargo run --release                          # GUI
cargo run --release -- --demo                # GUI con dati finti, nessun account
cargo run --release --bin ferrocord-tui      # terminale
cargo run --release --bin ferrocord-tui -- --demo
cargo test                                   # tutti i test (protocollo, voce, interfaccia)
```

Il binario finisce in `target/release/ferrocord` (`.exe` su Windows). Più
dettagli in [docs/sviluppo.md](docs/sviluppo.md).

## Uso

All'avvio compare la schermata di accesso:

- **QR code** (consigliato): apri l'app Discord sul telefono → Impostazioni →
  *Scansiona codice QR* e conferma. La password non passa mai dal client.
- **Token**: incolla il token e conferma. Il token viene verificato prima di
  essere salvato.

Nella GUI:

- clic su una **foto** per aprirla grande (rotellina per lo zoom, Esc per
  chiudere) e salvarla nella cartella Download;
- pulsanti **GIF** e **🙂** accanto al campo di scrittura: clic su una GIF per
  inviarla, clic su un'emoji per inserirla; le emoji del server si scrivono
  anche a mano come `:nome:`;
- clic su un **canale vocale** per entrare; nel pannello in basso a sinistra
  "Microphone" (spento all'ingresso), "Sound" per silenziare tutto e "Leave".
  Il codice a 30 cifre accanto al lucchetto è il codice di privacy della
  chiamata: è uguale per tutti i partecipanti e si può confrontare per
  verificare che nessuno si sia messo in mezzo.

### Opzioni e variabili d'ambiente

| | |
|---|---|
| `--demo` | dati finti, nessuna connessione a Discord |
| `--demo-login` | solo la schermata di accesso, con un QR finto |
| `--logout` | cancella il token salvato |
| `FERROCORD_TOKEN` | usa questo token invece di quello salvato |
| `FERROCORD_LOG` | livello del log, es. `debug` oppure `warn,ferrocord=debug` |

Il token viene salvato in:

| Sistema | Percorso |
|---|---|
| Linux | `~/.config/ferrocord/token` (permessi 600) |
| macOS | `~/Library/Application Support/dev.ferrocord.ferrocord/token` |
| Windows | `%APPDATA%\ferrocord\ferrocord\config\token` |

Il log va in un file (`%LOCALAPPDATA%\ferrocord\ferrocord\data\ferrocord.log`
su Windows, `~/.local/share/ferrocord/ferrocord.log` su Linux). Token e testo
dei messaggi non vengono mai scritti nel log.

### Tasti (terminale)

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

## Automazioni su GitHub

| Workflow | Quando | Cosa fa |
|---|---|---|
| **CI** | ogni push | formattazione, clippy, test e build su Windows, macOS e Linux; screenshot della GUI demo |
| **Installer Windows** | ogni push | installer Inno Setup (artifact); su `main` aggiorna la release **nightly** |
| **Sicurezza** | ogni push e ogni lunedì | `cargo audit`, `cargo deny`, Semgrep, Gitleaks; se trova falle importanti apre una issue e manda un'email |

Dettagli in [docs/sviluppo.md](docs/sviluppo.md) e
[docs/privacy-e-sicurezza.md](docs/privacy-e-sicurezza.md). L'installer non è
firmato digitalmente: al primo avvio Windows SmartScreen può chiedere conferma
(*Ulteriori informazioni → Esegui comunque*).

## Licenza

MIT
