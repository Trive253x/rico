//! Interface language: English (default) or Italian.
//!
//! Every text the user reads goes through [`t`] (fixed text) or [`tf!`]
//! (text with values), which pick the English or the Italian version.
//! The language is chosen at startup (`--lang`, `FERROCORD_LANG`, then the
//! saved setting) and can be changed from the GUI, which saves it.
//!
//! Internal error messages, logs and comments are English only.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    En,
    It,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::It];

    /// Code saved in the settings and accepted by `--lang`.
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::It => "it",
        }
    }

    /// Name of the language in the language itself.
    pub fn name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::It => "Italiano",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "en" | "eng" | "english" => Some(Lang::En),
            "it" | "ita" | "italian" | "italiano" => Some(Lang::It),
            _ => None,
        }
    }

    /// The text for this language.
    pub fn pick<T>(self, en: T, it: T) -> T {
        match self {
            Lang::En => en,
            Lang::It => it,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn lang() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::It,
        _ => Lang::En,
    }
}

pub fn set_lang(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

/// Fixed text in the current language.
pub fn t(en: &'static str, it: &'static str) -> &'static str {
    lang().pick(en, it)
}

/// Text with values in the current language:
/// `tf!("{n} messages", "{n} messaggi", n = count)`.
#[macro_export]
macro_rules! tf {
    ($en:literal, $it:literal $(, $($args:tt)*)?) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::En => format!($en $(, $($args)*)?),
            $crate::i18n::Lang::It => format!($it $(, $($args)*)?),
        }
    };
}

/// Month names for dates such as "3 November 2026".
pub fn month(lang: Lang, month: u32) -> &'static str {
    const EN: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    const IT: [&str; 12] = [
        "gennaio",
        "febbraio",
        "marzo",
        "aprile",
        "maggio",
        "giugno",
        "luglio",
        "agosto",
        "settembre",
        "ottobre",
        "novembre",
        "dicembre",
    ];
    let i = (month.clamp(1, 12) - 1) as usize;
    lang.pick(EN[i], IT[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_names() {
        for l in Lang::ALL {
            assert_eq!(Lang::parse(l.code()), Some(l));
            assert_eq!(Lang::parse(l.name()), Some(l));
        }
        assert_eq!(Lang::parse(" IT "), Some(Lang::It));
        assert_eq!(Lang::parse("fr"), None);
        assert_eq!(Lang::default(), Lang::En);
    }

    #[test]
    fn pick_and_months() {
        assert_eq!(Lang::It.pick("Leave", "Esci"), "Esci");
        assert_eq!(Lang::En.pick("Leave", "Esci"), "Leave");
        assert_eq!(month(Lang::En, 11), "November");
        assert_eq!(month(Lang::It, 9), "settembre");
        assert_eq!(month(Lang::It, 0), "gennaio");
    }
}
