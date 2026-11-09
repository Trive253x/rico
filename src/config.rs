//! Configuration paths, saved token and settings, logging to file.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use tracing_subscriber::EnvFilter;

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("dev", "ferrocord", "ferrocord")
}

/// Where the token is kept: the system keychain, under this name.
const KEYCHAIN_SERVICE: &str = "ferrocord";
const KEYCHAIN_ACCOUNT: &str = "discord-token";

/// File where older versions saved the token in clear. Read only once, to
/// move the token into the keychain, then deleted.
fn legacy_token_path() -> Option<PathBuf> {
    project_dirs().map(|d| d.config_dir().join("token"))
}

/// Somewhere a secret can be kept. The real one is the system keychain; the
/// tests use one in memory.
trait SecretStore {
    fn get(&self) -> Result<Option<String>>;
    fn set(&self, secret: &str) -> Result<()>;
    fn delete(&self) -> Result<()>;
}

/// The operating system keychain: Windows Credential Manager, macOS
/// Keychain, Linux kernel keyring.
struct Keychain;

impl Keychain {
    fn entry() -> Result<keyring::Entry> {
        keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).context("system keychain unavailable")
    }
}

impl SecretStore for Keychain {
    fn get(&self) -> Result<Option<String>> {
        match Self::entry()?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(anyhow::Error::new(e).context("reading the system keychain")),
        }
    }

    fn set(&self, secret: &str) -> Result<()> {
        Self::entry()?
            .set_password(secret)
            .context("saving in the system keychain")
    }

    fn delete(&self) -> Result<()> {
        match Self::entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(anyhow::Error::new(e).context("deleting from the system keychain")),
        }
    }
}

/// Where the token is saved, in words, for the login screen.
pub fn token_location() -> &'static str {
    use crate::i18n::t;
    if cfg!(windows) {
        t(
            "Windows Credential Manager (\"ferrocord\")",
            "Gestione credenziali di Windows (\"ferrocord\")",
        )
    } else if cfg!(target_os = "macos") {
        t(
            "the macOS Keychain (\"ferrocord\")",
            "il Portachiavi di macOS (\"ferrocord\")",
        )
    } else {
        t(
            "the Linux kernel keyring (until the next reboot)",
            "il portachiavi del kernel Linux (fino al prossimo riavvio)",
        )
    }
}

/// Token to use at startup: `FERROCORD_TOKEN` first, then the keychain.
pub fn load_token() -> Option<String> {
    if let Ok(t) = std::env::var("FERROCORD_TOKEN") {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    load_from(&Keychain, legacy_token_path().as_deref())
}

fn load_from(store: &dyn SecretStore, legacy: Option<&Path>) -> Option<String> {
    if let Some(path) = legacy.filter(|p| p.exists()) {
        if let Some(t) = migrate(store, path) {
            return Some(t);
        }
    }
    match store.get() {
        Ok(t) => t.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
        Err(e) => {
            tracing::warn!("token not read: {e:#}");
            None
        }
    }
}

/// Moves the token of an older version from the plain file into the
/// keychain and deletes the file. If the keychain does not work the file is
/// left where it is (the user did not choose to lose the login) and it is
/// tried again at the next start; nothing new is ever written in clear.
fn migrate(store: &dyn SecretStore, path: &Path) -> Option<String> {
    let token = std::fs::read_to_string(path).ok()?.trim().to_string();
    if token.is_empty() {
        wipe_file(path);
        return None;
    }
    match store.set(&token) {
        Ok(()) => {
            wipe_file(path);
            tracing::info!("token moved from the old file to the system keychain");
        }
        Err(e) => tracing::warn!("token still in the old file, the keychain did not work: {e:#}"),
    }
    Some(token)
}

/// Overwrites the file with zeros before deleting it (best effort: SSDs and
/// file system journals can still keep old copies).
fn wipe_file(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        if let Err(e) = std::fs::write(path, vec![0u8; meta.len() as usize]) {
            tracing::warn!("old token file not overwritten: {e}");
        }
    }
    if let Err(e) = std::fs::remove_file(path) {
        tracing::warn!("old token file not deleted: {e}");
    }
}

/// Saves the token in the system keychain. If that is not possible the
/// error goes back to the interface and the session goes on without being
/// saved: there is no fallback to a plain file.
pub fn save_token(token: &str) -> Result<()> {
    Keychain.set(token)
}

/// Deletes the saved token: from the keychain and, if still there, the old file.
pub fn delete_token() {
    delete_from(&Keychain, legacy_token_path().as_deref());
}

fn delete_from(store: &dyn SecretStore, legacy: Option<&Path>) {
    if let Err(e) = store.delete() {
        tracing::warn!("token not deleted: {e:#}");
    }
    if let Some(path) = legacy.filter(|p| p.exists()) {
        wipe_file(path);
    }
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

/// Folder of the settings files.
pub fn settings_dir() -> Option<PathBuf> {
    project_dirs().map(|d| d.config_dir().to_path_buf())
}

/// File with the client settings (see `settings`).
fn settings_path() -> Option<PathBuf> {
    project_dirs().map(|d| d.config_dir().join("settings.json"))
}

/// Saved settings, or the defaults.
pub fn load_settings() -> crate::settings::Settings {
    match settings_path().map(std::fs::read_to_string) {
        Some(Ok(text)) => crate::settings::Settings::from_json(&text),
        _ => crate::settings::Settings::default(),
    }
}

/// Saves the settings: written to a temporary file, then renamed, so a
/// crash never leaves half a file.
pub fn save_settings(settings: &crate::settings::Settings) -> Result<()> {
    let path = settings_path().context("configuration folder not found")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).context("creating the configuration folder")?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, settings.to_json()).context("saving the settings")?;
    std::fs::rename(&tmp, &path).context("saving the settings")?;
    Ok(())
}

/// Writes of one file from background threads, in the order they were
/// asked: one at a time, and a write asked before the last one done is
/// skipped (a slow write never puts back old values).
struct OrderedWrites {
    asked: AtomicU64,
    written: Mutex<u64>,
}

impl OrderedWrites {
    const fn new() -> Self {
        Self {
            asked: AtomicU64::new(0),
            written: Mutex::new(0),
        }
    }

    /// Takes the turn now (on the interface thread); the returned function
    /// does the write, on any thread.
    fn writer(
        &'static self,
        write: impl FnOnce() -> Result<()> + Send + 'static,
    ) -> impl FnOnce() -> Result<()> + Send + 'static {
        let turn = self.asked.fetch_add(1, Ordering::Relaxed) + 1;
        move || {
            let mut written = self.written.lock().unwrap_or_else(|e| e.into_inner());
            if turn < *written {
                return Ok(());
            }
            write()?;
            *written = turn;
            Ok(())
        }
    }
}

static SETTINGS_WRITES: OrderedWrites = OrderedWrites::new();
static LANGUAGE_WRITES: OrderedWrites = OrderedWrites::new();

/// [`save_settings`] to run off the interface thread (disk can be slow).
pub fn settings_writer(settings: crate::settings::Settings) -> impl FnOnce() -> Result<()> + Send + 'static {
    SETTINGS_WRITES.writer(move || save_settings(&settings))
}

/// [`save_language`] to run off the interface thread.
pub fn language_writer(lang: crate::i18n::Lang) -> impl FnOnce() -> Result<()> + Send + 'static {
    LANGUAGE_WRITES.writer(move || save_language(lang))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Keychain in memory; with `broken` every operation fails.
    #[derive(Default)]
    struct Memory {
        secret: RefCell<Option<String>>,
        broken: bool,
    }

    impl SecretStore for Memory {
        fn get(&self) -> Result<Option<String>> {
            anyhow::ensure!(!self.broken, "keychain unavailable");
            Ok(self.secret.borrow().clone())
        }
        fn set(&self, secret: &str) -> Result<()> {
            anyhow::ensure!(!self.broken, "keychain unavailable");
            *self.secret.borrow_mut() = Some(secret.to_string());
            Ok(())
        }
        fn delete(&self) -> Result<()> {
            anyhow::ensure!(!self.broken, "keychain unavailable");
            *self.secret.borrow_mut() = None;
            Ok(())
        }
    }

    #[test]
    fn an_older_write_never_wins() {
        static WRITES: OrderedWrites = OrderedWrites::new();
        let done = std::sync::Arc::new(Mutex::new(Vec::new()));
        let write = |n: u32| {
            let done = done.clone();
            WRITES.writer(move || {
                done.lock().unwrap().push(n);
                Ok(())
            })
        };
        let (first, second, third) = (write(1), write(2), write(3));
        // The second finishes first: the first, older, is skipped.
        second().unwrap();
        first().unwrap();
        third().unwrap();
        assert_eq!(*done.lock().unwrap(), [2, 3]);
    }

    fn legacy_file(name: &str, content: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("ferrocord-test-{}-{name}", std::process::id()));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn old_token_file_moves_into_the_keychain() {
        let store = Memory::default();
        let path = legacy_file("move", "  old-token\n");
        assert_eq!(load_from(&store, Some(&path)).as_deref(), Some("old-token"));
        assert_eq!(store.secret.borrow().as_deref(), Some("old-token"));
        assert!(!path.exists(), "the plain file must be gone");
        // Next start: from the keychain.
        assert_eq!(load_from(&store, Some(&path)).as_deref(), Some("old-token"));
    }

    #[test]
    fn broken_keychain_never_writes_in_clear() {
        let store = Memory {
            broken: true,
            ..Default::default()
        };
        // An old file stays (to try again later) and the session can start.
        let path = legacy_file("broken", "old-token");
        assert_eq!(load_from(&store, Some(&path)).as_deref(), Some("old-token"));
        assert!(path.exists());
        std::fs::remove_file(&path).unwrap();
        // Without an old file: nothing, and nothing is created.
        assert_eq!(load_from(&store, Some(&path)), None);
        assert!(!path.exists());
        assert!(store.set("new").is_err());
    }

    #[test]
    fn logout_deletes_keychain_and_old_file() {
        let store = Memory::default();
        store.set("token").unwrap();
        let path = legacy_file("logout", "token");
        delete_from(&store, Some(&path));
        assert_eq!(*store.secret.borrow(), None);
        assert!(!path.exists());
        // Empty old file: deleted, not migrated.
        let empty = legacy_file("empty", "  ");
        assert_eq!(load_from(&store, Some(&empty)), None);
        assert!(!empty.exists());
    }
}
