//! Push-to-talk: the key chosen in the settings, held to speak.
//!
//! In Rico's window the key comes from iced's keyboard events. Elsewhere, on
//! Windows, it is a system-wide hotkey through `global-hotkey` (the system's
//! `RegisterHotKey`): while it is registered Windows gives that key
//! combination to Rico only, so it is registered only during a call with
//! push-to-talk on, and let go as soon as either ends.

use iced::futures::Stream;
use iced::keyboard;

use super::Message;
use crate::audio::hotkey::{Hotkey, Modifiers};

/// The key of an iced keyboard event, by position (`KeyV`, `F13`).
pub fn code_of(physical: &keyboard::key::Physical) -> Option<String> {
    match physical {
        keyboard::key::Physical::Code(code) => Some(format!("{code:?}")),
        keyboard::key::Physical::Unidentified(_) => None,
    }
}

pub fn modifiers_of(m: keyboard::Modifiers) -> Modifiers {
    Modifiers {
        ctrl: m.control(),
        shift: m.shift(),
        alt: m.alt(),
    }
}

/// The system-wide hotkey (Windows only; elsewhere nothing is registered).
#[derive(Default)]
pub struct Global {
    #[cfg(windows)]
    manager: Option<global_hotkey::GlobalHotKeyManager>,
    #[cfg(windows)]
    registered: Option<global_hotkey::hotkey::HotKey>,
    /// The key registered now (saved form).
    key: Option<String>,
}

impl Global {
    /// The key registered now, if any.
    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    /// Registers `key` system-wide, or nothing (`None`). On failure (another
    /// program has that combination) the reason, in words; the key then
    /// works only in Rico's window.
    pub fn set(&mut self, key: Option<&Hotkey>) -> Result<(), String> {
        let wanted = key.map(ToString::to_string);
        if wanted == self.key {
            return Ok(());
        }
        self.clear();
        let Some(key) = key else { return Ok(()) };
        self.register(key)
    }

    #[cfg(windows)]
    fn register(&mut self, key: &Hotkey) -> Result<(), String> {
        let hotkey: global_hotkey::hotkey::HotKey = key
            .to_string()
            .parse()
            .map_err(|e| tf!("Unknown key: {e}", "Tasto sconosciuto: {e}"))?;
        if self.manager.is_none() {
            self.manager = Some(
                global_hotkey::GlobalHotKeyManager::new()
                    .map_err(|e| tf!("Push-to-talk: {e}", "Push-to-talk: {e}"))?,
            );
        }
        let Some(manager) = self.manager.as_ref() else {
            return Ok(());
        };
        match manager.register(hotkey) {
            Ok(()) => {
                tracing::info!("push-to-talk: key registered");
                self.registered = Some(hotkey);
                self.key = Some(key.to_string());
                Ok(())
            }
            Err(e) => {
                tracing::warn!("push-to-talk: key not registered: {e}");
                Err(tf!(
                    "Push-to-talk: {} is used by another program, it works only in Rico's window ({e}).",
                    "Push-to-talk: {} è usato da un altro programma, funziona solo nella finestra di Rico ({e}).",
                    key.label()
                ))
            }
        }
    }

    #[cfg(not(windows))]
    fn register(&mut self, _key: &Hotkey) -> Result<(), String> {
        Ok(())
    }

    fn clear(&mut self) {
        #[cfg(windows)]
        if let (Some(manager), Some(hotkey)) = (self.manager.as_ref(), self.registered.take()) {
            let _ = manager.unregister(hotkey);
            tracing::info!("push-to-talk: key released");
        }
        self.key = None;
    }
}

impl Drop for Global {
    fn drop(&mut self) {
        self.clear();
    }
}

/// The system-wide key held and let go, as messages (only while a key is
/// registered: see [`Global`]).
pub fn events() -> impl Stream<Item = Message> {
    iced::stream::channel(
        16,
        |mut out: iced::futures::channel::mpsc::Sender<Message>| async move {
            #[cfg(windows)]
            {
                use iced::futures::SinkExt;
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                global_hotkey::GlobalHotKeyEvent::set_event_handler(Some(
                    move |e: global_hotkey::GlobalHotKeyEvent| {
                        let _ = tx.send(e.state == global_hotkey::HotKeyState::Pressed);
                    },
                ));
                while let Some(held) = rx.recv().await {
                    if out.send(Message::PushToTalk(held)).await.is_err() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_keys_by_position() {
        let pressed = |code, m| {
            Hotkey::from_press(
                &code_of(&keyboard::key::Physical::Code(code)).unwrap(),
                modifiers_of(m),
            )
        };
        let k = pressed(keyboard::key::Code::KeyV, keyboard::Modifiers::CTRL).unwrap();
        assert_eq!(k.to_string(), "ctrl+KeyV");
        assert_eq!(
            pressed(keyboard::key::Code::F13, keyboard::Modifiers::empty())
                .unwrap()
                .label(),
            "F13"
        );
        assert!(pressed(keyboard::key::Code::Escape, keyboard::Modifiers::empty()).is_none());
        let mut g = Global::default();
        assert!(g.set(None).is_ok());
        assert_eq!(g.key(), None);
    }
}
