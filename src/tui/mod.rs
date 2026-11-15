//! Terminal interface (ratatui), started by `rico-tui`.
//!
//! Every network task sends `AppEvent`s on a single channel: the main loop
//! applies them to the state and redraws only when something happens.

pub mod app;
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

/// Starts the terminal interface (must be called inside a tokio runtime).
pub async fn run(demo: bool) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut app = if demo {
        App::demo(tx)
    } else {
        App::new(tx, crate::config::load_token())
    };

    // ratatui::init also installs a panic hook that restores the terminal.
    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    let result = event_loop(&mut terminal, &mut app, &mut rx).await;
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    rx: &mut mpsc::UnboundedReceiver<AppEvent>,
) -> Result<()> {
    let mut term_events = EventStream::new();
    // The tick is only for animations (spinner) and fires only when there are
    // some: at rest the process never wakes up.
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

        // Bursts of events (e.g. many incoming messages) => a single redraw.
        while let Ok(ev) = rx.try_recv() {
            app.handle(ev);
        }
    }
    app.shutdown();
    Ok(())
}
