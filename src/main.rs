//! ferrocord — client Discord alternativo per il terminale.
//!
//! Architettura in breve:
//! - `discord/`: protocollo (gateway WebSocket, API REST, login con QR)
//! - `app.rs`: stato dell'applicazione e gestione degli eventi
//! - `ui/`: disegno dell'interfaccia con ratatui
//!
//! Tutti i task di rete inviano `AppEvent` su un unico canale: il loop
//! principale li applica allo stato e ridisegna solo quando succede qualcosa.

mod app;
mod config;
mod demo;
mod discord;
mod input;
mod ui;

use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste, EventStream},
    execute,
};
use futures_util::StreamExt;
use tokio::{sync::mpsc, time::MissedTickBehavior};

use app::{App, AppEvent};

const HELP: &str = "\
ferrocord — client Discord alternativo per il terminale

Uso:
  ferrocord            avvia il client (login con QR code o token)
  ferrocord --demo     interfaccia con dati finti, senza connettersi a Discord
  ferrocord --logout   cancella il token salvato ed esce
  ferrocord --help     mostra questo aiuto

Variabili d'ambiente:
  FERROCORD_TOKEN      usa questo token invece di quello salvato
  FERROCORD_LOG        livello di log (default: info), es. debug
";

#[tokio::main]
async fn main() -> Result<()> {
    let mut demo = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--demo" => demo = true,
            "--logout" => {
                config::delete_token();
                println!("Token rimosso.");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("ferrocord {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            other => {
                eprintln!("Argomento sconosciuto: {other}\n\n{HELP}");
                std::process::exit(2);
            }
        }
    }

    // Più librerie TLS potrebbero portarsi dietro provider crittografici
    // diversi: fissiamo ring come provider di processo per non avere sorprese.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let _log_guard = config::init_logging();

    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut app = if demo {
        App::demo(tx)
    } else {
        App::new(tx, config::load_token())
    };

    // ratatui::init installa anche un panic hook che ripristina il terminale.
    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    let result = run(&mut terminal, &mut app, &mut rx).await;
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    rx: &mut mpsc::UnboundedReceiver<AppEvent>,
) -> Result<()> {
    let mut term_events = EventStream::new();
    // Il tick serve solo alle animazioni (spinner) e scatta solo quando ce ne
    // sono: a riposo il processo non si sveglia mai.
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    app.start();
    while app.running {
        terminal.draw(|f| ui::draw(f, app))?;

        let event = tokio::select! {
            Some(ev) = rx.recv() => ev,
            ev = term_events.next() => match ev {
                Some(Ok(ev)) => AppEvent::Term(ev),
                Some(Err(e)) => return Err(e.into()),
                None => break,
            },
            _ = tick.tick(), if app.is_animating() => AppEvent::Tick,
        };
        app.handle(event);

        // Raffiche di eventi (es. molti messaggi in arrivo) => un solo redraw.
        while let Ok(ev) = rx.try_recv() {
            app.handle(ev);
        }
    }
    app.shutdown();
    Ok(())
}
