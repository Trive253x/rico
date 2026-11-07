//! ferrocord — a lightweight, native alternative Discord client.
//!
//! - `discord/`: protocol (WebSocket gateway, REST API, QR login, voice)
//! - `store`: session data shared by the two interfaces
//! - `gui/`: graphical interface (iced), the main program
//! - `tui/`: terminal interface (ratatui), started by `ferrocord-tui`
//! - `audio/`: Opus codec, microphone and speakers
//! - `i18n`: interface language (English or Italian)

// First, so that `tf!` is visible in every module below.
#[macro_use]
pub mod i18n;

pub mod audio;
pub mod config;
pub mod demo;
pub mod discord;
pub mod gui;
pub mod media;
pub mod store;
pub mod text;
pub mod tui;

/// Command line options shared by both programs.
#[derive(Debug, Default, Clone, Copy)]
pub struct Options {
    pub demo: bool,
    /// GUI only: shows the login screen with a fake QR code (for screenshots).
    pub demo_login: bool,
}

fn help() -> &'static str {
    i18n::t(
        "\
ferrocord — alternative Discord client

Usage:
  ferrocord            graphical interface
  ferrocord-tui        terminal interface

Options:
  --demo               fake data, no connection to Discord
  --lang en|it         interface language (saved for the next start)
  --logout             delete the saved token and exit
  --version            show the version
  --help               show this help

Environment variables:
  FERROCORD_TOKEN      use this token instead of the saved one
  FERROCORD_LANG       interface language for this run (en, it)
  FERROCORD_LOG        log level (default: info), e.g. debug
",
        "\
ferrocord — client Discord alternativo

Uso:
  ferrocord            interfaccia grafica
  ferrocord-tui        interfaccia da terminale

Opzioni:
  --demo               dati finti, nessuna connessione a Discord
  --lang en|it         lingua dell'interfaccia (salvata per i prossimi avvii)
  --logout             cancella il token salvato ed esce
  --version            mostra la versione
  --help               mostra questo aiuto

Variabili d'ambiente:
  FERROCORD_TOKEN      usa questo token invece di quello salvato
  FERROCORD_LANG       lingua dell'interfaccia per questo avvio (en, it)
  FERROCORD_LOG        livello di log (default: info), es. debug
",
    )
}

/// Reads the arguments; `Err(code)` if the program has to exit right away.
pub fn parse_args() -> Result<Options, i32> {
    // Language first, so that help and errors are already translated:
    // `--lang` (saved), then `FERROCORD_LANG`, then the saved setting.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env_lang = std::env::var("FERROCORD_LANG")
        .ok()
        .and_then(|l| i18n::Lang::parse(&l));
    i18n::set_lang(env_lang.or_else(config::load_language).unwrap_or_default());

    let mut opts = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--demo" => opts.demo = true,
            "--demo-login" => opts.demo_login = true,
            "--lang" => match args.next().as_deref().and_then(i18n::Lang::parse) {
                Some(lang) => {
                    i18n::set_lang(lang);
                    if let Err(e) = config::save_language(lang) {
                        eprintln!("{e:#}");
                    }
                }
                None => {
                    eprintln!(
                        "{}\n\n{}",
                        i18n::t("--lang wants en or it", "--lang vuole en o it"),
                        help()
                    );
                    return Err(2);
                }
            },
            "--logout" => {
                config::delete_token();
                println!("{}", i18n::t("Token removed.", "Token rimosso."));
                return Err(0);
            }
            "-V" | "--version" => {
                println!("ferrocord {}", env!("CARGO_PKG_VERSION"));
                return Err(0);
            }
            "-h" | "--help" => {
                print!("{}", help());
                return Err(0);
            }
            other => {
                eprintln!(
                    "{}\n\n{}",
                    tf!("Unknown argument: {other}", "Argomento sconosciuto: {other}"),
                    help()
                );
                return Err(2);
            }
        }
    }
    Ok(opts)
}

/// Setup shared by both programs; keep the guard alive until exit.
pub fn init() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    // Several TLS libraries could bring different crypto providers: ring is
    // set as the process-wide provider to avoid surprises.
    let _ = rustls::crypto::ring::default_provider().install_default();
    config::init_logging()
}
