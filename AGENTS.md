# AGENTS.md

Istruzioni per gli assistenti AI (e per chiunque) che lavorano su ferrocord.
Le parti tra `<!-- MODIFICA: … -->` sono da completare o cambiare: il resto
descrive come si è lavorato finora e si può correggere liberamente.

## Il progetto in breve

ferrocord è un client Discord alternativo in Rust, con GUI (iced) e interfaccia
da terminale (ratatui) sugli stessi dati. Obiettivo: un'esperienza più leggera
e fluida del client ufficiale.

<!-- MODIFICA: obiettivi del progetto, a chi è destinato, cosa NON deve diventare -->

Prima di modificare il codice leggi:

- [docs/architettura.md](docs/architettura.md): moduli e flusso dei dati;
- [docs/protocollo-discord.md](docs/protocollo-discord.md): cosa sappiamo del
  protocollo e come è stato verificato;
- [docs/sviluppo.md](docs/sviluppo.md): comandi, test, CI, branch.

## Regole

1. **Account Discord.** Le prove si fanno in modalità demo (`--demo`). Non
   usare l'account del proprietario se non lo chiede esplicitamente, e non
   inviare mai messaggi, reazioni o GIF senza un permesso esplicito per quel
   canale.
2. **Niente automazioni** dell'account (invii automatici, scraping, self-bot):
   vanno contro i Termini di Servizio di Discord.
3. **Segreti.** Mai token, password o contenuto dei messaggi nel log, nel codice,
   nei test o nei commit. Nel log solo eventi tecnici e numeri.
4. **Git.** Niente `push --force` né riscritture della storia su branch
   pubblicati: gli errori si correggono con un nuovo commit. Un branch
   `feat/…` per ogni funzione, merge in `main` con pull request e merge commit.
5. **Prima di ogni push**: `cargo fmt`, `cargo clippy --all-targets -- -D
   warnings` e `cargo test` devono passare; per le modifiche all'interfaccia,
   una prova in demo (meglio con uno screenshot).
6. **Dipendenze nuove**: solo se servono davvero; aggiorna `Cargo.lock` e
   controlla che le licenze passino `cargo deny`.

<!-- MODIFICA: altre regole (es. cosa chiedere prima di fare, limiti di tempo, budget) -->

## Come lavorare

- Flusso: implementare, compilare, verificare (test + demo), fare push, poi
  **fermarsi** e riassumere cosa è cambiato, cosa è stato provato e cosa no.
- Si compila e si prova sul PC Windows "Portatile Casa" (`C:\Users\Trive\ferrocord`),
  non sul portatile di lavoro.
- Se una scelta cambia il comportamento per l'utente o tocca la privacy (es.
  contattare un server nuovo), segnalarla nel riepilogo.

<!-- MODIFICA: quanto spesso aggiornarti, quando chiederti conferma, formato dei riepiloghi -->

## Stile

- Lingua: italiano per commenti, messaggi all'utente, nomi dei test, commit e
  documentazione.
- Commit: titolo breve al presente ("Aggiunge…" o descrittivo), poi un elenco
  di cosa cambia e perché.
- Codice: `rustfmt.toml` (righe da 110); modelli dati tolleranti
  (`#[serde(default)]`); rete separata dall'interfaccia; download solo tramite
  `media::fetch`.
- Interfaccia: tema One Dark in `gui/style.rs`, stile "gpui" (pulito, compatto).

<!-- MODIFICA: preferenze di stile e di interfaccia (colori, densità, scorciatoie) -->

## Priorità

Dove si è arrivati e cosa viene dopo (vedi anche la tabella in README.md):

1. Chat vocale (`feat/chat-vocale`): connessione e trasporto fatti; poi audio
   Opus, microfono e altoparlanti, crittografia end-to-end DAVE.
2. Permessi per canale (nascondere i canali non leggibili).
3. Rispondere, reagire e caricare file dalla GUI.
4. Avatar veri, anteprime dei link, messaggi "card" dei bot.

<!-- MODIFICA: riordina, aggiungi o togli priorità -->

## Da non toccare senza chiedere

- `.github/workflows/` e `.github/scripts/` (CI, installer, sicurezza);
- `installer/ferrocord.iss`;
- `.cargo/audit.toml` (le eccezioni di sicurezza vanno motivate);
- la release `nightly`.

<!-- MODIFICA: altri file o aree delicate -->

## Note per l'ambiente

- Su Windows chiudere ferrocord prima di `cargo build --release`.
- Il log è in `%LOCALAPPDATA%\ferrocord\ferrocord\data\ferrocord.log`;
  `FERROCORD_LOG=warn,ferrocord=debug` per i dettagli.
- Ambienti senza accesso a crates.io non possono compilare: in quel caso si
  modifica il codice e si compila sul PC Windows.

<!-- MODIFICA: strumenti installati, percorsi, account di test, altro -->
