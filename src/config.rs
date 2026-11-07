//! Percorsi di configurazione, salvataggio del token e logging su file.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use tracing_subscriber::EnvFilter;

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("dev", "ferrocord", "ferrocord")
}

/// File in cui viene salvato il token (es. `~/.config/ferrocord/token` su Linux).
pub fn token_path() -> Option<PathBuf> {
    project_dirs().map(|d| d.config_dir().join("token"))
}

/// Token da usare all'avvio: prima `FERROCORD_TOKEN`, poi il file salvato.
pub fn load_token() -> Option<String> {
    if let Ok(t) = std::env::var("FERROCORD_TOKEN") {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    let t = std::fs::read_to_string(token_path()?).ok()?;
    let t = t.trim().to_string();
    (!t.is_empty()).then_some(t)
}

pub fn save_token(token: &str) -> Result<PathBuf> {
    let path = token_path().context("cartella di configurazione non trovata")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_private(&path, token.as_bytes())?;
    Ok(path)
}

pub fn delete_token() {
    if let Some(p) = token_path() {
        let _ = std::fs::remove_file(p);
    }
}

/// Scrive un file leggibile solo dall'utente corrente (0600 su Unix).
#[cfg(unix)]
fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // Se il file esisteva già con permessi più larghi, li restringiamo.
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(data)?;
    Ok(())
}

/// Su Windows il profilo utente è già privato per impostazione predefinita.
#[cfg(not(unix))]
fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    std::fs::write(path, data)?;
    Ok(())
}

pub fn log_dir() -> Option<PathBuf> {
    project_dirs().map(|d| d.data_local_dir().to_path_buf())
}

/// Il log va su file: stdout è occupato dall'interfaccia.
/// Attenzione a non loggare mai token o payload completi del gateway.
pub fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let dir = log_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let appender = tracing_appender::rolling::never(&dir, "ferrocord.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let mut filter = EnvFilter::try_from_env("FERROCORD_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    // The encryption libraries log key material at trace level (davey logs
    // the per-user base secrets): cap them whatever FERROCORD_LOG says.
    for directive in ["davey=info", "openmls=warn", "openmls_rust_crypto=warn"] {
        if let Ok(d) = directive.parse() {
            filter = filter.add_directive(d);
        }
    }
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter(filter)
        .init();
    Some(guard)
}
