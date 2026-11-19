//! Windows notifications for new messages (toast notifications), and the
//! click on one, which opens its channel.
//!
//! Which messages notify is decided by the store (`Store::should_notify`,
//! from the account's settings) and by the interface (not the channel being
//! looked at). Here: showing them, off the interface thread, a few at a time.
//!
//! Windows shows a toast under an application ID registered for the current
//! user (`HKCU\Software\Classes\AppUserModelId\Trive253x.Rico`: the name
//! "Rico" and its icon); it is written the first time a notification is
//! shown.

use iced::futures::Stream;

use super::Message;

/// What a notification says.
#[derive(Debug, Clone)]
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Toast {
    pub channel_id: String,
    /// Who wrote, and where.
    pub title: String,
    /// The message text (or what it carries), shortened.
    pub body: String,
}

/// Longest text in a notification (characters).
pub const MAX_BODY: usize = 200;

/// The text shortened to [`MAX_BODY`] characters, on one line.
pub fn short(text: &str) -> String {
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= MAX_BODY {
        return one_line;
    }
    let mut s: String = one_line.chars().take(MAX_BODY - 1).collect();
    s.push('…');
    s
}

/// Shows a notification (Windows; elsewhere nothing).
pub fn show(toast: Toast) {
    #[cfg(windows)]
    windows::show(toast);
    #[cfg(not(windows))]
    let _ = toast;
}

/// Clicks on notifications, as messages to open the channel.
pub fn events() -> impl Stream<Item = Message> {
    iced::stream::channel(
        16,
        |mut out: iced::futures::channel::mpsc::Sender<Message>| async move {
            #[cfg(windows)]
            {
                use iced::futures::SinkExt;
                let mut rx = windows::clicks();
                while let Some(channel_id) = rx.recv().await {
                    if out.send(Message::NotificationClicked(channel_id)).await.is_err() {
                        break;
                    }
                }
            }
            #[cfg(not(windows))]
            {
                let _ = &mut out;
                std::future::pending::<()>().await;
            }
        },
    )
}

#[cfg(windows)]
mod windows {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

    use super::Toast;

    /// The application ID notifications are shown under.
    const APP_ID: &str = "Trive253x.Rico";
    /// Notifications being handed to Windows at once, at most.
    const MAX_IN_FLIGHT: usize = 4;

    static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
    static REGISTERED: OnceLock<bool> = OnceLock::new();
    static CLICKS: Mutex<Option<UnboundedSender<String>>> = Mutex::new(None);

    /// Where the clicks go (the newest subscription).
    pub fn clicks() -> UnboundedReceiver<String> {
        let (tx, rx) = unbounded_channel();
        if let Ok(mut slot) = CLICKS.lock() {
            *slot = Some(tx);
        }
        rx
    }

    /// The name and icon Windows shows for Rico's notifications.
    fn register() -> bool {
        let result = (|| -> anyhow::Result<()> {
            let key = windows_registry::CURRENT_USER
                .create(format!(r"Software\Classes\AppUserModelId\{APP_ID}"))?;
            key.set_string("DisplayName", "Rico")?;
            if let Some(dir) = crate::config::log_dir() {
                let icon = dir.join("notification-icon.png");
                std::fs::create_dir_all(&dir)?;
                std::fs::write(&icon, include_bytes!("../../assets/icon.png"))?;
                key.set_string("IconUri", icon.to_string_lossy().as_ref())?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("notifications: application ID not registered: {e:#}");
                false
            }
        }
    }

    pub fn show(toast: Toast) {
        if IN_FLIGHT.fetch_add(1, Ordering::AcqRel) >= MAX_IN_FLIGHT {
            IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("notification".into())
            .spawn(move || {
                let id = if *REGISTERED.get_or_init(register) {
                    APP_ID
                } else {
                    tauri_winrt_notification::Toast::POWERSHELL_APP_ID
                };
                let channel_id = toast.channel_id.clone();
                let shown = tauri_winrt_notification::Toast::new(id)
                    .title(&toast.title)
                    .text1(&toast.body)
                    .sound(Some(tauri_winrt_notification::Sound::Default))
                    .on_activated(move |_| {
                        if let Ok(slot) = CLICKS.lock() {
                            if let Some(tx) = slot.as_ref() {
                                let _ = tx.send(channel_id.clone());
                            }
                        }
                        Ok(())
                    })
                    .show();
                if let Err(e) = shown {
                    tracing::warn!("notifications: not shown: {e}");
                }
                IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
            });
        if spawned.is_err() {
            IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_texts_are_shortened_on_one_line() {
        assert_eq!(short("hi\n  there"), "hi there");
        let long = "a".repeat(500);
        let s = short(&long);
        assert_eq!(s.chars().count(), MAX_BODY);
        assert!(s.ends_with('…'));
    }
}
