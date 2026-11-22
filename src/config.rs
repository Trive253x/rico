//! Configuration paths, saved token and settings, logging to file.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use tracing_subscriber::EnvFilter;

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("dev", "rico", "rico")
}

/// The folders of the versions called ferrocord (up to 0.12), moved to the
/// new ones at the first start.
fn old_project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("dev", "ferrocord", "ferrocord")
}

/// Where the token is kept: the system keychain, under this name.
const KEYCHAIN_SERVICE: &str = "rico";
/// The name used up to 0.12: a token saved there is moved under the new one.
const OLD_KEYCHAIN_SERVICE: &str = "ferrocord";
const KEYCHAIN_ACCOUNT: &str = "discord-token";

/// An environment variable: `RICO_<name>`, or `FERROCORD_<name>` as up to 0.12.
pub fn env_var(name: &str) -> Option<String> {
    std::env::var(format!("RICO_{name}"))
        .or_else(|_| std::env::var(format!("FERROCORD_{name}")))
        .ok()
}

/// Folders moved from the old name (and failed moves), for the log: they
/// are moved before the log exists.
static FOLDERS_MOVED: AtomicUsize = AtomicUsize::new(0);
static FOLDERS_NOT_MOVED: AtomicUsize = AtomicUsize::new(0);

/// Moves the settings and data of the versions called ferrocord to the new
/// folders, once: only where the new folder does not exist yet. To call
/// before anything reads or writes them.
pub fn migrate_old_folders() {
    let (Some(old), Some(new)) = (old_project_dirs(), project_dirs()) else {
        return;
    };
    let pairs = [
        (old.config_dir(), new.config_dir()),
        (old.data_dir(), new.data_dir()),
        (old.data_local_dir(), new.data_local_dir()),
    ];
    for (from, to) in pairs {
        match move_folder(from, to) {
            Moved::Yes => FOLDERS_MOVED.fetch_add(1, Ordering::Relaxed),
            Moved::Failed => FOLDERS_NOT_MOVED.fetch_add(1, Ordering::Relaxed),
            Moved::Nothing => 0,
        };
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Moved {
    Yes,
    Nothing,
    Failed,
}

/// Renames `from` to `to` if `from` exists and `to` does not, then removes
/// the old parent folders left empty (only those of the old name).
fn move_folder(from: &Path, to: &Path) -> Moved {
    if !from.is_dir() || to.exists() {
        return Moved::Nothing;
    }
    if let Some(parent) = to.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return Moved::Failed;
        }
    }
    if std::fs::rename(from, to).is_err() {
        return Moved::Failed;
    }
    // On Windows the folders are `…\ferrocord\ferrocord\config`: the
    // empty `ferrocord` ones stay behind. `remove_dir` only removes empty ones.
    let mut parent = from.parent();
    while let Some(dir) = parent.filter(|d| d.file_name().is_some_and(|n| n == "ferrocord")) {
        if std::fs::remove_dir(dir).is_err() {
            break;
        }
        parent = dir.parent();
    }
    Moved::Yes
}

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
struct Keychain(&'static str);

/// The keychain entry of this version and the one of the versions called ferrocord.
const KEYCHAIN: Keychain = Keychain(KEYCHAIN_SERVICE);
const OLD_KEYCHAIN: Keychain = Keychain(OLD_KEYCHAIN_SERVICE);

impl Keychain {
    fn entry(&self) -> Result<keyring::Entry> {
        keyring::Entry::new(self.0, KEYCHAIN_ACCOUNT).context("system keychain unavailable")
    }
}

impl SecretStore for Keychain {
    fn get(&self) -> Result<Option<String>> {
        match self.entry()?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(anyhow::Error::new(e).context("reading the system keychain")),
        }
    }

    fn set(&self, secret: &str) -> Result<()> {
        self.entry()?
            .set_password(secret)
            .context("saving in the system keychain")
    }

    fn delete(&self) -> Result<()> {
        match self.entry()?.delete_credential() {
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
            "Windows Credential Manager (\"rico\")",
            "Gestione credenziali di Windows (\"rico\")",
        )
    } else if cfg!(target_os = "macos") {
        t(
            "the macOS Keychain (\"rico\")",
            "il Portachiavi di macOS (\"rico\")",
        )
    } else {
        t(
            "the Linux kernel keyring (until the next reboot)",
            "il portachiavi del kernel Linux (fino al prossimo riavvio)",
        )
    }
}

/// Token to use at startup: `RICO_TOKEN` (or `FERROCORD_TOKEN`) first, then
/// the keychain.
pub fn load_token() -> Option<String> {
    if let Some(t) = env_var("TOKEN") {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    load_from(&KEYCHAIN, &OLD_KEYCHAIN, legacy_token_path().as_deref())
}

fn load_from(store: &dyn SecretStore, old: &dyn SecretStore, legacy: Option<&Path>) -> Option<String> {
    if let Some(path) = legacy.filter(|p| p.exists()) {
        if let Some(t) = migrate(store, path) {
            return Some(t);
        }
    }
    match store.get() {
        Ok(Some(t)) if !t.trim().is_empty() => Some(t.trim().to_string()),
        Ok(_) => rename_entry(store, old),
        Err(e) => {
            tracing::warn!("token not read: {e:#}");
            None
        }
    }
}

/// Moves the token saved under the old name (versions called ferrocord)
/// to the new one. The old entry is deleted only once the new one is saved;
/// if that fails the token is used anyway and the move is tried again at
/// the next start.
fn rename_entry(store: &dyn SecretStore, old: &dyn SecretStore) -> Option<String> {
    let token = match old.get() {
        Ok(Some(t)) => t.trim().to_string(),
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!("token under the old name not read: {e:#}");
            return None;
        }
    };
    if token.is_empty() {
        return None;
    }
    match store.set(&token) {
        Ok(()) => {
            if let Err(e) = old.delete() {
                tracing::warn!("token under the old name not deleted: {e:#}");
            }
            tracing::info!("token moved to the keychain entry of the new name");
        }
        Err(e) => tracing::warn!("token left under the old name, the keychain did not work: {e:#}"),
    }
    Some(token)
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
    KEYCHAIN.set(token)
}

/// Deletes the saved token: from the keychain (also under the old name) and,
/// if still there, the old file.
pub fn delete_token() {
    delete_from(&KEYCHAIN, &OLD_KEYCHAIN, legacy_token_path().as_deref());
}

fn delete_from(store: &dyn SecretStore, old: &dyn SecretStore, legacy: Option<&Path>) {
    for s in [store, old] {
        if let Err(e) = s.delete() {
            tracing::warn!("token not deleted: {e:#}");
        }
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
            let mut written = self
                .written
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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

/// A log bigger than this at start is moved to `rico.old.log` (replacing
/// the previous one) and a new one begins.
const LOG_KEEP: u64 = 5 * 1024 * 1024;
/// At most this much is written in one run: a library repeating an error
/// on every frame once filled 1.35 GB.
const LOG_PER_RUN: u64 = 20 * 1024 * 1024;

/// The log file with a size limit per run; past it one line says so and
/// the rest is dropped.
struct CappedLog {
    file: std::fs::File,
    written: u64,
    limit: u64,
    full: bool,
}

impl std::io::Write for CappedLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.full {
            return Ok(buf.len());
        }
        if self.written + buf.len() as u64 > self.limit {
            self.full = true;
            let _ = self
                .file
                .write_all(b"--- log limit reached for this run: nothing more is written ---\n");
            return Ok(buf.len());
        }
        self.written += buf.len() as u64;
        self.file.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Opens `rico.log` in `dir` for appending, after moving a too big one to
/// `rico.old.log`.
fn open_log(dir: &Path, keep: u64, per_run: u64) -> Option<CappedLog> {
    let path = dir.join("rico.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > keep) {
        let _ = std::fs::rename(&path, dir.join("rico.old.log"));
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    Some(CappedLog {
        file,
        written: 0,
        limit: per_run,
        full: false,
    })
}

/// The log goes to a file: stdout belongs to the interface.
/// Never log tokens or full gateway payloads.
pub fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let dir = log_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let appender = open_log(&dir, LOG_KEEP, LOG_PER_RUN)?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let mut filter = env_var("LOG")
        .and_then(|l| EnvFilter::try_new(l).ok())
        .unwrap_or_else(|| EnvFilter::new("info"));
    // The encryption libraries log key material at trace level (davey logs
    // the per-user base secrets): cap them whatever RICO_LOG says.
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
    let (moved, failed) = (
        FOLDERS_MOVED.load(Ordering::Relaxed),
        FOLDERS_NOT_MOVED.load(Ordering::Relaxed),
    );
    if moved > 0 {
        tracing::info!("{moved} folders moved from the old name");
    }
    if failed > 0 {
        tracing::warn!("{failed} folders of the old name not moved");
    }
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
        let path = std::env::temp_dir().join(format!("rico-test-{}-{name}", std::process::id()));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn old_token_file_moves_into_the_keychain() {
        let store = Memory::default();
        let path = legacy_file("move", "  old-token\n");
        assert_eq!(
            load_from(&store, &Memory::default(), Some(&path)).as_deref(),
            Some("old-token")
        );
        assert_eq!(store.secret.borrow().as_deref(), Some("old-token"));
        assert!(!path.exists(), "the plain file must be gone");
        // Next start: from the keychain.
        assert_eq!(
            load_from(&store, &Memory::default(), Some(&path)).as_deref(),
            Some("old-token")
        );
    }

    #[test]
    fn broken_keychain_never_writes_in_clear() {
        let store = Memory {
            broken: true,
            ..Default::default()
        };
        // An old file stays (to try again later) and the session can start.
        let path = legacy_file("broken", "old-token");
        assert_eq!(
            load_from(&store, &Memory::default(), Some(&path)).as_deref(),
            Some("old-token")
        );
        assert!(path.exists());
        std::fs::remove_file(&path).unwrap();
        // Without an old file: nothing, and nothing is created.
        assert_eq!(load_from(&store, &Memory::default(), Some(&path)), None);
        assert!(!path.exists());
        assert!(store.set("new").is_err());
    }

    #[test]
    fn logout_deletes_keychain_and_old_file() {
        let store = Memory::default();
        store.set("token").unwrap();
        let path = legacy_file("logout", "token");
        delete_from(&store, &Memory::default(), Some(&path));
        assert_eq!(*store.secret.borrow(), None);
        assert!(!path.exists());
        // Empty old file: deleted, not migrated.
        let empty = legacy_file("empty", "  ");
        assert_eq!(load_from(&store, &Memory::default(), Some(&empty)), None);
        assert!(!empty.exists());
    }

    #[test]
    fn token_under_the_old_name_moves_to_the_new_one() {
        let (store, old) = (Memory::default(), Memory::default());
        old.set(" old-name-token ").unwrap();
        assert_eq!(load_from(&store, &old, None).as_deref(), Some("old-name-token"));
        assert_eq!(store.secret.borrow().as_deref(), Some("old-name-token"));
        assert_eq!(*old.secret.borrow(), None, "the old entry must be gone");
        // A token under the new name wins over an old one.
        old.set("stale").unwrap();
        assert_eq!(load_from(&store, &old, None).as_deref(), Some("old-name-token"));
        // Logout removes both.
        delete_from(&store, &old, None);
        assert_eq!(
            (store.secret.borrow().clone(), old.secret.borrow().clone()),
            (None, None)
        );
    }

    #[test]
    fn old_name_token_is_kept_if_the_keychain_fails() {
        let store = Memory {
            broken: true,
            ..Default::default()
        };
        let old = Memory::default();
        old.set("t").unwrap();
        // The new entry cannot be read: nothing is touched.
        assert_eq!(load_from(&store, &old, None), None);
        assert_eq!(old.secret.borrow().as_deref(), Some("t"));
    }

    #[test]
    fn the_log_is_capped_and_rotated() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("rico-test-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Too big at start: moved aside, a new one begins.
        std::fs::write(dir.join("rico.log"), vec![b'x'; 200]).unwrap();
        let mut log = open_log(&dir, 100, 50).unwrap();
        assert_eq!(std::fs::metadata(dir.join("rico.old.log")).unwrap().len(), 200);
        assert_eq!(log.write(&[b'a'; 30]).unwrap(), 30);
        assert_eq!(log.write(&[b'b'; 30]).unwrap(), 30, "accepted, not written");
        assert_eq!(log.write(&[b'c'; 30]).unwrap(), 30);
        log.flush().unwrap();
        let text = std::fs::read_to_string(dir.join("rico.log")).unwrap();
        assert!(text.starts_with(&"a".repeat(30)));
        assert!(text.contains("log limit reached"));
        assert!(!text.contains("bbb") && !text.contains("ccc"));
        // Small enough: kept and appended to.
        drop(log);
        let mut log = open_log(&dir, 1000, 1000).unwrap();
        log.write_all(b"more").unwrap();
        assert!(std::fs::read_to_string(dir.join("rico.log"))
            .unwrap()
            .ends_with("more"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn old_folders_move_once() {
        let root = std::env::temp_dir().join(format!("rico-test-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let from = root.join("ferrocord").join("ferrocord").join("config");
        let to = root.join("rico").join("rico").join("config");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("settings.json"), "{}").unwrap();
        assert_eq!(move_folder(&from, &to), Moved::Yes);
        assert_eq!(std::fs::read_to_string(to.join("settings.json")).unwrap(), "{}");
        // The empty folders of the old name are gone, the rest stays.
        assert!(!root.join("ferrocord").exists());
        assert!(root.exists());
        // Again: nothing to move. With both present the new one is kept.
        assert_eq!(move_folder(&from, &to), Moved::Nothing);
        std::fs::create_dir_all(&from).unwrap();
        assert_eq!(move_folder(&from, &to), Moved::Nothing);
        assert!(from.exists() && to.join("settings.json").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
