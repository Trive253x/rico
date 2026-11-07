# Sviluppo

## Prerequisiti

- Rust stabile ≥ 1.85 ([rustup.rs](https://rustup.rs));
- su Windows i Build Tools di Visual Studio (C++), per il linker, con il
  componente CMake per compilare libopus (se `cmake` non è nel PATH, la
  variabile `CMAKE` deve puntare a `cmake.exe`);
- per l'installer, solo in CI: Inno Setup.

Il PC di sviluppo di riferimento è Windows ("Portatile Casa"): lì si compila,
si provano la GUI e la demo. Gli ambienti senza accesso a crates.io non possono
compilare.

## Comandi

```sh
cargo fmt                                    # formattazione (rustfmt.toml: righe da 110)
cargo clippy --all-targets -- -D warnings    # nessun avviso ammesso
cargo test                                   # tutti i test
cargo test -- --include-ignored              # anche i test manuali (saltano se manca l'input)
cargo build --release                        # binari ottimizzati
cargo run --release -- --demo                # GUI demo
```

CI usa `--locked`: dopo aver aggiunto una dipendenza, `Cargo.lock` va
committato.

Su Windows, prima di `cargo build --release`, chiudere ferrocord: se
l'eseguibile è in uso la compilazione fallisce con "Accesso negato".

## Provare senza account

**Tutte le prove si fanno in modalità demo** (`--demo`), non con un account
Discord vero. La demo copre: canali e messaggi, invio, immagini e vista a
schermo intero, GIF animate (anche dei link), selettore di GIF ed emoji, emoji
del server, chat vocale (server vocale finto in locale: due partecipanti DAVE
veri che parlano a turno con una voce sintetica, cifrata end-to-end; il
microfono resta spento finché non si preme "Microphone").

Le prove con un account vero si fanno solo quando chi possiede l'account lo
chiede esplicitamente, e mai inviando messaggi senza il suo permesso.

## Test

| Dove | Cosa provano |
|---|---|
| `discord/*` | parsing dei payload, zlib, token, query |
| `discord/voice/*` | cifratura (le due modalità, estensioni RTP, pacchetti alterati), RTP, IP discovery; DAVE (versione 0 rifiutata, messaggi malformati); chiamata completa al server finto con cifratura end-to-end: codice di privacy, audio Opus in entrata e in uscita, microfono spento = nulla inviato, commit quando gli altri entrano dopo di noi, frame senza DAVE scartati, chiamata senza DAVE rifiutata, seconda cifratura, 4017 definitivo, riconnessione; indirizzi del server vocale |
| `audio/*` | Opus andata e ritorno, input errati, code limitate, ricampionamento, conversione dei canali, mixer, rilevamento della voce |
| `media.rs` | server ammessi, indirizzi delle GIF, embed, decodifica di GIF e WebP animate, nomi di file sicuri |
| `text.rs` | tag di Discord, emoji personalizzate, conversione `:nome:` all'invio |
| `store.rs` | cronologia, reazioni, non letti, stati vocali |
| `gui/*` | flussi dell'interfaccia in demo: navigazione, invio, anteprime, vista grande, selettore, emoji del server, voce |

Test manuale su file veri: `FERROCORD_ANIM="a.gif;b.webp" cargo test --lib
animazione_da_file -- --ignored --nocapture` decodifica le animazioni indicate.

## Log

`FERROCORD_LOG=warn,ferrocord=debug` mostra download, anteprime, connessioni e
voce. Il file è in `%LOCALAPPDATA%\ferrocord\ferrocord\data\ferrocord.log` su
Windows. Nel log non vanno mai token, testo dei messaggi o link inviati.

## Branch e merge

- `main` è sempre stabile: a ogni push pubblica la release nightly.
- Ogni funzione ha il suo branch (`feat/…`); si unisce a `main` con una pull
  request e un merge commit (`Merge: … (#n)`).
- Niente `push --force` né riscritture della storia su branch già pubblicati:
  gli errori si correggono con un nuovo commit.
- Messaggi di commit in italiano: titolo breve, poi un elenco di cosa cambia e
  perché.

| Branch | Stato |
|---|---|
| `main` | GUI, immagini, GIF, emoji (PR #1 e #7) |
| `feat/chat-vocale` | chat vocale: trasporto, audio Opus, microfono e altoparlanti, DAVE (da provare con Discord) |

## CI

| Workflow | File | Note |
|---|---|---|
| CI | `.github/workflows/ci.yml` | matrice Windows/macOS/Linux: `fmt --check`, clippy, test, build; screenshot della demo su Linux con Xvfb |
| Installer Windows | `.github/workflows/installer.yml` | Inno Setup (`installer/ferrocord.iss`), versione `x.y.z.<numero esecuzione>`; release `nightly` solo da `main` |
| Sicurezza | `.github/workflows/security.yml` | vedi [privacy-e-sicurezza.md](privacy-e-sicurezza.md) |

Girano solo sui push (non anche sulle pull request, che mostrano i risultati
dello stesso commit) e un push nuovo sullo stesso branch annulla le esecuzioni
precedenti. Dependabot propone gli aggiornamenti di dipendenze e azioni.

## Convenzioni del codice

- Commenti, messaggi all'utente e nomi dei test in italiano.
- Modelli dati con `#[serde(default)]` sui campi opzionali: un campo mancante
  non deve far fallire un evento intero.
- Il codice di rete non dipende dall'interfaccia: eventi su un canale, comandi
  tramite un handle.
- Ogni download passa da `media::fetch`, che ammette solo i server dell'elenco
  e limita la dimensione.
- I widget che compaiono e scompaiono stanno in livelli dello `stack` radice,
  per non far perdere lo stato agli altri (vedi
  [architettura.md](architettura.md)).
