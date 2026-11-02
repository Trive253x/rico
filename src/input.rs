//! Campo di testo su una riga, con cursore e gestione corretta dell'Unicode.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Default, Clone)]
pub struct TextInput {
    value: String,
    /// Posizione del cursore in caratteri (non in byte).
    cursor: usize,
}

impl TextInput {
    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    pub fn set(&mut self, s: &str) {
        self.clear();
        self.insert_str(s);
    }

    fn len_chars(&self) -> usize {
        self.value.chars().count()
    }

    fn byte_idx(&self, char_idx: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.value.len())
    }

    pub fn insert_char(&mut self, c: char) {
        let i = self.byte_idx(self.cursor);
        self.value.insert(i, c);
        self.cursor += 1;
    }

    /// Inserisce testo incollato: è un campo su una riga, gli a capo diventano spazi.
    pub fn insert_str(&mut self, s: &str) {
        let clean: String = s
            .chars()
            .filter(|c| *c != '\r')
            .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
            .filter(|c| !c.is_control())
            .collect();
        let i = self.byte_idx(self.cursor);
        self.value.insert_str(i, &clean);
        self.cursor += clean.chars().count();
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            let i = self.byte_idx(self.cursor);
            self.value.remove(i);
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.len_chars() {
            let i = self.byte_idx(self.cursor);
            self.value.remove(i);
        }
    }

    fn delete_word_back(&mut self) {
        let chars: Vec<char> = self.value.chars().collect();
        let mut start = self.cursor;
        while start > 0 && chars[start - 1] == ' ' {
            start -= 1;
        }
        while start > 0 && chars[start - 1] != ' ' {
            start -= 1;
        }
        let (a, b) = (self.byte_idx(start), self.byte_idx(self.cursor));
        self.value.replace_range(a..b, "");
        self.cursor = start;
    }

    /// Gestisce un tasto; restituisce `true` se il campo l'ha usato.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        // AltGr su Windows arriva come Ctrl+Alt: va trattato come testo normale,
        // altrimenti con la tastiera italiana non si scrivono @, # e [ ].
        let ctrl =
            key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.len_chars(),
            KeyCode::Char('u') if ctrl => self.clear(),
            KeyCode::Char('w') if ctrl => self.delete_word_back(),
            KeyCode::Char(_) if ctrl => return false,
            KeyCode::Char(c) => self.insert_char(c),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.len_chars()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.len_chars(),
            _ => return false,
        }
        true
    }

    /// Porzione di testo visibile in `width` colonne e colonna del cursore.
    /// Con `mask` ogni carattere viene sostituito (campi password/token).
    pub fn visible(&self, width: usize, mask: Option<char>) -> (String, u16) {
        let chars: Vec<char> = match mask {
            Some(m) => self.value.chars().map(|_| m).collect(),
            None => self.value.chars().collect(),
        };
        let w = |c: &char| c.width().unwrap_or(0);
        let width = width.max(1);

        // Scorri a destra finché il cursore non entra nello spazio disponibile.
        let mut start = 0;
        while start < self.cursor && chars[start..self.cursor].iter().map(w).sum::<usize>() > width - 1 {
            start += 1;
        }

        let mut out = String::new();
        let mut used = 0;
        for c in &chars[start..] {
            let cw = w(c);
            if used + cw > width {
                break;
            }
            out.push(*c);
            used += cw;
        }
        let cursor_x = chars[start..self.cursor].iter().map(w).sum::<usize>();
        (out, cursor_x as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn edita_con_unicode() {
        let mut t = TextInput::default();
        for c in "ciaò".chars() {
            t.handle_key(key(KeyCode::Char(c)));
        }
        t.handle_key(key(KeyCode::Left));
        t.handle_key(key(KeyCode::Backspace));
        assert_eq!(t.value(), "ciò");
        t.handle_key(key(KeyCode::End));
        t.insert_str(" a\ntutti");
        assert_eq!(t.value(), "ciò a tutti");
    }

    #[test]
    fn altgr_scrive_caratteri() {
        let mut t = TextInput::default();
        let altgr = KeyEvent::new(KeyCode::Char('@'), KeyModifiers::CONTROL | KeyModifiers::ALT);
        assert!(t.handle_key(altgr));
        assert_eq!(t.value(), "@");
    }

    #[test]
    fn visibile_segue_il_cursore() {
        let mut t = TextInput::default();
        t.set("0123456789");
        let (s, x) = t.visible(5, None);
        assert_eq!(s, "6789");
        assert_eq!(x, 4);
        let (s, _) = t.visible(20, Some('•'));
        assert_eq!(s, "••••••••••");
    }
}
