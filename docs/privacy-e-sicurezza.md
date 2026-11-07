# Privacy e sicurezza

## Token

- Si ottiene con il login QR (la password non passa mai da ferrocord) oppure
  incollandolo; nel secondo caso viene verificato prima di salvarlo.
- È salvato in un file leggibile solo dall'utente (permessi 600 su Linux e
  macOS, cartella del profilo su Windows). `ferrocord --logout` lo cancella.
- Nelle richieste HTTP l'intestazione `Authorization` è marcata come sensibile,
  così non finisce nei log di debug delle librerie.
- Non viene mai scritto nel log, né mostrato nell'interfaccia.

## Log

Il log contiene solo eventi tecnici: connessioni, errori, numeri (server,
messaggi, dimensioni dei download, pacchetti audio). Non contiene token, testo
dei messaggi, link inviati né contenuti dei canali.

## Server contattati

| Server | Perché |
|---|---|
| `gateway.discord.gg`, `discord.com` | gateway e API |
| `remote-auth-gateway.discord.gg` | login con QR |
| `cdn.discordapp.com`, `media.discordapp.net`, `images-ext-1/2.discordapp.net` | allegati, emoji, immagini e GIF tramite il proxy di Discord |
| `media.tenor.com`, `media1.tenor.com` | GIF dei link Tenor |
| `media.giphy.com`, `i.giphy.com`, `media0-4.giphy.com` | GIF dei link Giphy |
| `static.klipy.com` | anteprime del selettore GIF |
| server vocali `*.discord.media` (WebSocket e UDP) | chat vocale, solo dopo aver cliccato un canale vocale |

`media::fetch` scarica solo da questi server (HTTPS), con limiti di dimensione
(8 MB per le anteprime, 25 MB per la vista grande, 600 MB per i file salvati).

**Il tuo indirizzo IP è visibile a terzi in due casi**, perché Discord non
offre una copia tramite il suo proxy:

- le GIF dei link Tenor e Giphy si scaricano dai server di Tenor (Google) e
  Giphy;
- le anteprime nel selettore GIF si scaricano da Klipy.

Le GIF di Klipy inviate nei messaggi invece passano dal proxy di Discord.

## File scaricati

- Il nome del file viene ripulito (niente percorsi, caratteri vietati su
  Windows, nomi riservati) e reso unico nella cartella Download.
- Il download va su un file `.part`, rinominato solo a download completo.

## Decodifica di immagini e animazioni

Limiti contro file costruiti per consumare memoria: GIF e WebP animate al
massimo 4096 px per lato, 300 fotogrammi e 64 MB una volta decodificate; la
decodifica avviene fuori dal thread dell'interfaccia.

## Chat vocale

- L'audio viaggia cifrato tra ferrocord e il server vocale di Discord
  (AES-256-GCM o XChaCha20-Poly1305). I pacchetti alterati o con la chiave
  sbagliata vengono scartati e contati.
- Dentro il trasporto, ogni frame audio è cifrato **end-to-end** con DAVE:
  Discord non può ascoltare. Se il server non attiva DAVE, o chiede di
  tornare alla versione 0, la chiamata viene chiusa; i frame che non si
  decifrano con DAVE vengono scartati, mai riprodotti.
- Il codice di privacy a 30 cifre nel pannello della voce è uguale per tutti i
  partecipanti: confrontarlo (a voce o altrove) conferma che nessuno si è messo
  in mezzo.
- **Microfono**: si apre solo con un clic su "Microphone", che resta rosso
  finché è acceso. Spegnerlo, silenziare l'audio, uscire dal canale o chiudere
  l'app chiude il dispositivo. Finché la cifratura end-to-end non è pronta non
  parte nulla.
- Audio, chiavi e payload non finiscono mai nel log né su disco. I log di
  `davey` e `openmls` sono limitati a `info`/`warn` anche con
  `FERROCORD_LOG=trace`, perché a livello `trace` scriverebbero segreti.
- L'indirizzo del server vocale viene accettato solo come `wss://` con un nome
  host valido; `ws://` solo verso `127.0.0.1` (il server finto della demo).
- In demo il server vocale finto ascolta solo su `127.0.0.1`; gli altoparlanti
  riproducono voci sintetiche e il microfono resta spento finché non lo accendi.
- Opus è decodificato da libopus (C, tramite il crate `opus`), con pacchetti
  limitati a 1275 byte e al massimo 64 decoder contemporanei.

## Controlli automatici

Il workflow **Sicurezza** gira a ogni push e ogni lunedì:

- `cargo audit`: vulnerabilità note nelle dipendenze (database RustSec);
- `cargo deny`: licenze, provenienza e duplicati delle dipendenze;
- Semgrep: analisi statica del codice Rust e dei workflow;
- Gitleaks: token o password finiti nel repository, anche nella cronologia.

Se trova **falle importanti** (vulnerabilità nota, risultato Semgrep grave,
segreto nel repository, controllo che non è riuscito a girare) apre o aggiorna
una issue con l'etichetta `sicurezza` e fa fallire l'esecuzione: GitHub manda
un'email per entrambe. Se l'esecuzione viene annullata da un push successivo
non segnala nulla (i controlli ripartono sul commit nuovo).

Gli avvisi minori (librerie non più mantenute, duplicati) finiscono nel
riepilogo dell'esecuzione. Le segnalazioni ignorate di proposito sono in
`.cargo/audit.toml`, ognuna con il motivo.
