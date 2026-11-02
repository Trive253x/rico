//! ferrocord con interfaccia grafica.

// In release su Windows niente finestra di console dietro la GUI.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> iced::Result {
    let opts = match ferrocord::parse_args() {
        Ok(o) => o,
        Err(code) => std::process::exit(code),
    };
    let _log = ferrocord::init();
    ferrocord::gui::run(opts)
}
