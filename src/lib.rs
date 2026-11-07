//! ferrocord — client Discord alternativo, nativo e leggero.
//!
//! - `discord/`: protocollo (gateway WebSocket, API REST, login con QR)
//! - `store`: dati della sessione condivisi tra le due interfacce
//! - `gui/`: interfaccia grafica (iced), il programma principale
//! - `tui/`: interfaccia da terminale (ratatui), avviata da `ferrocord-tui`

pub mod audio;
pub mod config;
pub mod demo;
pub mod discord;
pub mod gui;
pub mod media;
pub mod store;
pub mod text;
pub mod tui;

/// Opzioni comuni da riga di comando.
#[derive(Debug, Default, Clone, Copy)]
pub struct Options {
    pub demo: bool,
    /// Solo GUI: mostra la schermata di login con un QR finto (per gli screenshot).
    pub demo_login: bool,
}

pub const HELP: &str = "\
ferrocord — client Discord alternativo

Uso:
  ferrocord            interfaccia grafica
  ferrocord-tui        interfaccia da terminale

Opzioni:
  --demo               dati finti, nessuna connessione a Discord
  --logout             cancella il token salvato ed esce
  --version            mostra la versione
  --help               mostra questo aiuto

Variabili d'ambiente:
  FERROCORD_TOKEN      usa questo token invece di quello salvato
  FERROCORD_LOG        livello di log (default: info), es. debug
";

/// Legge gli argomenti; `Err(codice)` se il programma deve uscire subito.
pub fn parse_args() -> Result<Options, i32> {
    let mut opts = Options::default();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--demo" => opts.demo = true,
            "--demo-login" => opts.demo_login = true,
            "--logout" => {
                config::delete_token();
                println!("Token rimosso.");
                return Err(0);
            }
            "-V" | "--version" => {
                println!("ferrocord {}", env!("CARGO_PKG_VERSION"));
                return Err(0);
            }
            "-h" | "--help" => {
                print!("{HELP}");
                return Err(0);
            }
            other => {
                eprintln!("Argomento sconosciuto: {other}\n\n{HELP}");
                return Err(2);
            }
        }
    }
    Ok(opts)
}

/// Impostazioni comuni a entrambi i programmi; il guard va tenuto vivo fino all'uscita.
pub fn init() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    // Più librerie TLS potrebbero portarsi dietro provider crittografici
    // diversi: fissiamo ring come provider di processo per non avere sorprese.
    let _ = rustls::crypto::ring::default_provider().install_default();
    config::init_logging()
}
