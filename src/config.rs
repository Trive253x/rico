//! Configuration paths, saved token and settings, logging to file.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use tracing_subscriber::EnvFilter;

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("dev", "ferrocord", "ferrocord")
}

/// File where the token is saved (e.g. `~/.config/ferrocord/token` on Linux).
pub fn token_path() -> Option<PathBuf> {
    project_dirs().map(|d| d.config_dir().join("token"))
}

/// Token to use at startup: `FERROCORD_TOKEN` first, then the saved file.
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
    let path = token_path().context("configuration folder not found")?;
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

/// Writes a file readable only by the current user (0600 on Unix).
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
    // If the file already existed with wider permissions, narrow them.
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(data)?;
    Ok(())
}

/// On Windows the user profile is already private by default.
#[cfg(not(unix))]
fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    std::fs::write(path, data)?;
    Ok(())
}

/// File with the interface language (`en` or `it`).
fn language_path() -> Option<PathBuf> {
    project_dirs().map(|d| d.config_dir().join("language"))
}

/// Saved interface language, if any.
pub fn load_language() -> Option<crate::i18n::Lang> {
    let s = std::fs::read_to_string(language_path()?).ok()?;
    crate::i18n::Lang::parse(&s)
}

pub fn save_language(lang: crate::i18n::Lang) -> Result<()> {
    let path = language_path().context("configuration folder not found")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, lang.code()).context("saving the language")?;
    Ok(())
}

pub fn log_dir() -> Option<PathBuf> {
    project_dirs().map(|d| d.data_local_dir().to_path_buf())
}

/// The log goes to a file: stdout belongs to the interface.
/// Never log tokens or full gateway payloads.
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
