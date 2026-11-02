# ferrocord

Client Discord alternativo scritto in Rust: nativo, leggero, senza Electron né
Chromium. Due interfacce sugli stessi dati:

- **`ferrocord`**: interfaccia grafica (iced), quella installata dall'installer;
- **`ferrocord-tui`**: interfaccia da terminale (ratatui).

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
| Risposte (citazione), reazioni, allegati, "sta scrivendo…" (solo visualizzazione) | ✅ |
| GUI iced: barra server, canali, messaggi con avatar, tema One Dark | ✅ |
| Modalità demo senza rete (`--demo`) | ✅ |
| Installer Windows a ogni push, controlli di sicurezza automatici | ✅ |
| Permessi per canale (oggi si vedono anche canali non accessibili) | ⏳ |
| Rispondere, reagire, caricare file dall'interfaccia | ⏳ |
| Anteprime delle immagini e avatar veri | ⏳ |
| Voce | ⏳ |

## Compilare

Serve Rust stabile ≥ 1.82 ([rustup.rs](https://rustup.rs)). Su Windows servono
anche i Build Tools di Visual Studio (li chiede già l'installer di Rust).

```sh
cargo run --release                          # GUI
cargo run --release -- --demo                # GUI con dati finti, nessuna connessione
cargo run --release --bin ferrocord-tui      # terminale
cargo run --release --bin ferrocord-tui -- --demo
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

Il log va in un file (lo standard output è occupato dall'interfaccia):
`~/.local/share/ferrocord/ferrocord.log` su Linux. Il livello si regola con
`FERROCORD_LOG=debug`. Token e payload non vengono mai scritti nel log.

## Architettura

```
src/
├── main.rs            programma `ferrocord` (GUI)
├── bin/ferrocord-tui.rs   programma `ferrocord-tui` (terminale)
├── lib.rs             opzioni comuni, avvio
├── store.rs           dati della sessione condivisi dalle due interfacce
├── text.rs            menzioni e tag di Discord resi leggibili
├── config.rs          percorsi, salvataggio del token, logging su file
├── demo.rs            dati finti per --demo
├── discord/
│   ├── gateway.rs     WebSocket: identify/resume, heartbeat, eventi
│   ├── zlib.rs        decompressione zlib-stream del gateway
│   ├── rest.rs        API REST: cronologia, invio, profilo, rate limit
│   ├── remote_auth.rs login con QR (RSA-OAEP + ticket)
│   └── models.rs      modelli dati tolleranti ai campi mancanti
├── gui/               interfaccia grafica (iced): stato, update, view, stili
└── tui/               interfaccia da terminale (ratatui)
```

Il codice di rete non conosce le interfacce: gateway e login QR mandano i loro
eventi su un canale e ciascuna interfaccia li converte nei propri messaggi.
Nella GUI girano come *sottoscrizioni* di iced, che le avvia e le ferma da sola
quando cambia lo stato (login, logout, nuova sessione).

Scelte che contano per le prestazioni:

- `compress=zlib-stream` sul gateway: il READY di un account utente pesa
  diversi MB non compressi;
- il payload del gateway viene letto una volta sola: `op`/`t` subito, il campo
  `d` resta grezzo (`RawValue`) e viene deserializzato direttamente nel tipo
  finale;
- i messaggi vengono tenuti in memoria solo per i canali aperti;
- la vista dei messaggi è ancorata in basso: i messaggi nuovi e quelli più
  vecchi caricati scorrendo non fanno saltare la posizione.

## Automazioni su GitHub

| Workflow | Quando | Cosa fa |
|---|---|---|
| **CI** | ogni push e pull request | formattazione, clippy, test e build su Windows, macOS e Linux; screenshot della GUI demo (artifact `screenshot`) |
| **Installer Windows** | ogni push | compila l'installer Inno Setup (artifact `ferrocord-setup-…`); su `main` aggiorna anche la release **nightly** |
| **Sicurezza** | ogni push, pull request e ogni lunedì | `cargo audit` (vulnerabilità note nelle dipendenze), `cargo deny` (licenze e provenienza), Semgrep (codice e workflow), Gitleaks (token finiti nel repository) |

Se il workflow **Sicurezza** trova falle importanti (una vulnerabilità nota,
un risultato Semgrep di gravità alta, un segreto nel repository, oppure un
controllo che non è riuscito a girare) apre o aggiorna una issue con
l'etichetta `sicurezza` menzionando il proprietario del repository e fa fallire
l'esecuzione: GitHub manda un'email per entrambe le cose. Perché arrivino,
nelle impostazioni di GitHub (*Settings → Notifications*) devono essere attive
le email per le issue dei repository che segui e per i workflow falliti.
L'elenco delle segnalazioni ignorate di proposito, con il motivo, è in
`.cargo/audit.toml`.

L'installer non è firmato digitalmente: al primo avvio Windows SmartScreen può
chiedere conferma (*Ulteriori informazioni → Esegui comunque*).

## Prossimi passi consigliati

1. Calcolo dei permessi (ruoli + override del canale) per nascondere i canali
   non leggibili.
2. Rispondere e reagire dalla GUI (servono le chiamate REST e i pulsanti).
3. Avatar e anteprime delle immagini, con cache su disco.
4. Cache su disco di server e canali per un avvio istantaneo.

## Licenza

MIT
