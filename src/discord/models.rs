//! Modelli dati minimi. Tutti i campi opzionali hanno `#[serde(default)]`:
//! i payload di Discord per gli account utente cambiano spesso forma e un
//! campo mancante non deve far fallire la deserializzazione.

use std::cmp::Ordering;

use serde::Deserialize;

/// Tipi di canale (campo `type`).
pub mod kind {
    pub const GUILD_TEXT: u8 = 0;
    pub const DM: u8 = 1;
    pub const GUILD_VOICE: u8 = 2;
    pub const GROUP_DM: u8 = 3;
    pub const CATEGORY: u8 = 4;
    pub const ANNOUNCEMENT: u8 = 5;
    pub const STAGE: u8 = 13;
}

/// Confronta due snowflake (ID numerici in stringa) senza convertirli.
pub fn cmp_snowflake(a: &str, b: &str) -> Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
}

impl User {
    pub fn display_name(&self) -> &str {
        self.global_name
            .as_deref()
            .filter(|n| !n.is_empty())
            .unwrap_or(&self.username)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Channel {
    pub id: String,
    #[serde(rename = "type", default)]
    pub kind: u8,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub position: Option<i64>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub guild_id: Option<String>,
    #[serde(default)]
    pub last_message_id: Option<String>,
    #[serde(default)]
    pub recipients: Vec<User>,
    /// Alcune versioni del READY mandano solo gli ID dei destinatari.
    #[serde(default)]
    pub recipient_ids: Vec<String>,
}

impl Channel {
    pub fn is_text(&self) -> bool {
        matches!(
            self.kind,
            kind::GUILD_TEXT | kind::DM | kind::GROUP_DM | kind::ANNOUNCEMENT
        )
    }

    pub fn is_voice(&self) -> bool {
        matches!(self.kind, kind::GUILD_VOICE | kind::STAGE)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GuildProperties {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Guild {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Con alcune "capabilities" il nome sta qui invece che in `name`.
    #[serde(default)]
    pub properties: Option<GuildProperties>,
    #[serde(default)]
    pub channels: Vec<Channel>,
}

impl Guild {
    pub fn name(&self) -> &str {
        self.name
            .as_deref()
            .or_else(|| self.properties.as_ref().and_then(|p| p.name.as_deref()))
            .unwrap_or("…")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Attachment {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub url: String,
    /// Copia sul media proxy di Discord: accetta `width`/`height` per le miniature.
    #[serde(default)]
    pub proxy_url: String,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

impl Attachment {
    /// Immagine statica o animata che si può mostrare in anteprima.
    pub fn is_image(&self) -> bool {
        let by_type = self
            .content_type
            .as_deref()
            .is_some_and(|t| t.starts_with("image/") && t != "image/svg+xml");
        let lower = self.filename.to_ascii_lowercase();
        let by_name = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp"]
            .iter()
            .any(|ext| lower.ends_with(ext));
        // Senza dimensioni Discord non l'ha riconosciuta come immagine.
        (by_type || by_name) && self.width.unwrap_or(0) > 0 && self.height.unwrap_or(0) > 0
    }
}

/// Emoji di una reazione: Unicode (`name` è il carattere) o personalizzata (`id` presente).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct Emoji {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

impl Emoji {
    /// Testo da mostrare: il carattere Unicode, oppure `:nome:` per le emoji personalizzate.
    pub fn label(&self) -> String {
        match (&self.id, &self.name) {
            (Some(_), Some(name)) => format!(":{name}:"),
            (None, Some(name)) => name.clone(),
            _ => "?".to_string(),
        }
    }

    pub fn same(&self, other: &Emoji) -> bool {
        match (&self.id, &other.id) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.name == other.name,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Reaction {
    #[serde(default)]
    pub count: u32,
    #[serde(default)]
    pub me: bool,
    #[serde(default)]
    pub emoji: Emoji,
}

/// Il messaggio a cui si risponde: ne servono solo autore e testo.
#[derive(Debug, Clone, Deserialize)]
pub struct MessagePreview {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub author: Option<User>,
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    pub author: User,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub edited_timestamp: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub mentions: Vec<User>,
    #[serde(default)]
    pub mention_everyone: bool,
    #[serde(rename = "type", default)]
    pub kind: u8,
    #[serde(default)]
    pub reactions: Vec<Reaction>,
    #[serde(default)]
    pub referenced_message: Option<Box<MessagePreview>>,
    /// Anteprime generate da Discord per i link (GIF di Tenor/Giphy, immagini…).
    #[serde(default)]
    pub embeds: Vec<Embed>,
}

/// Anteprima di un link creata da Discord. Per i link a GIF il tipo è `gifv`
/// (con `video` in MP4 e `thumbnail` statica), per i link a immagini `image`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Embed {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub provider: Option<EmbedProvider>,
    #[serde(default)]
    pub thumbnail: Option<EmbedMedia>,
    #[serde(default)]
    pub image: Option<EmbedMedia>,
    #[serde(default)]
    pub video: Option<EmbedMedia>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbedProvider {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbedMedia {
    #[serde(default)]
    pub url: String,
    /// Copia sul proxy di Discord (images-ext-*.discordapp.net).
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

/// MESSAGE_UPDATE può contenere solo i campi cambiati.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageUpdate {
    pub id: String,
    pub channel_id: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub edited_timestamp: Option<String>,
    #[serde(default)]
    pub attachments: Option<Vec<Attachment>>,
    /// Discord aggiunge le anteprime dei link con un MESSAGE_UPDATE successivo.
    #[serde(default)]
    pub embeds: Option<Vec<Embed>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Member {
    #[serde(default)]
    pub user: Option<User>,
    #[serde(default)]
    pub nick: Option<String>,
}

/// TYPING_START: qualcuno sta scrivendo in un canale.
#[derive(Debug, Clone, Deserialize)]
pub struct TypingStart {
    pub channel_id: String,
    pub user_id: String,
    #[serde(default)]
    pub member: Option<Member>,
}

/// MESSAGE_REACTION_ADD / MESSAGE_REACTION_REMOVE.
#[derive(Debug, Clone, Deserialize)]
pub struct ReactionEvent {
    pub user_id: String,
    pub channel_id: String,
    pub message_id: String,
    #[serde(default)]
    pub emoji: Emoji,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageDelete {
    pub id: String,
    pub channel_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Ready {
    pub user: User,
    pub session_id: String,
    #[serde(default)]
    pub resume_gateway_url: Option<String>,
    #[serde(default)]
    pub guilds: Vec<Guild>,
    #[serde(default)]
    pub private_channels: Vec<Channel>,
    #[serde(default)]
    pub users: Vec<User>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messaggio_con_risposta_e_reazioni() {
        let json = r#"{
            "id": "5", "channel_id": "1", "content": "ok",
            "author": {"id": "2", "username": "giulia"},
            "referenced_message": {"id": "4", "content": "domanda?", "author": {"id": "3", "username": "marco"}},
            "reactions": [
                {"count": 2, "me": true, "emoji": {"id": null, "name": "👍"}},
                {"count": 1, "me": false, "emoji": {"id": "99", "name": "ferris"}}
            ]
        }"#;
        let m: Message = serde_json::from_str(json).unwrap();
        assert_eq!(m.referenced_message.as_ref().unwrap().content, "domanda?");
        assert_eq!(m.reactions[0].emoji.label(), "👍");
        assert_eq!(m.reactions[1].emoji.label(), ":ferris:");
        // Una risposta a un messaggio eliminato ha `referenced_message: null`.
        let m: Message = serde_json::from_str(
            r#"{"id":"6","channel_id":"1","author":{"id":"2"},"referenced_message":null}"#,
        )
        .unwrap();
        assert!(m.referenced_message.is_none());
    }

    #[test]
    fn embed_gif_di_tenor() {
        let json = r#"{
            "id": "7", "channel_id": "1", "content": "https://tenor.com/view/gatto-12345",
            "author": {"id": "2", "username": "giulia"},
            "embeds": [{
                "type": "gifv", "url": "https://tenor.com/view/gatto-12345",
                "provider": {"name": "Tenor", "url": "https://tenor.co"},
                "thumbnail": {"url": "https://media.tenor.com/abcAAAAe/gatto.png",
                    "proxy_url": "https://images-ext-1.discordapp.net/external/x/https/media.tenor.com/abcAAAAe/gatto.png",
                    "width": 498, "height": 280, "placeholder": "x", "flags": 0},
                "video": {"url": "https://media.tenor.com/abcAAAPo/gatto.mp4", "width": 498, "height": 280}
            }]
        }"#;
        let m: Message = serde_json::from_str(json).unwrap();
        let e = &m.embeds[0];
        assert_eq!(e.kind.as_deref(), Some("gifv"));
        assert_eq!(
            e.video.as_ref().unwrap().url,
            "https://media.tenor.com/abcAAAPo/gatto.mp4"
        );
        assert_eq!(e.provider.as_ref().unwrap().name.as_deref(), Some("Tenor"));
    }

    #[test]
    fn allegati_immagine() {
        let img: Attachment = serde_json::from_str(
            r#"{"id":"1","filename":"Foto.JPG","size":10,"url":"https://cdn.discordapp.com/a","proxy_url":"https://media.discordapp.net/a","width":800,"height":600}"#,
        )
        .unwrap();
        assert!(img.is_image());
        let svg: Attachment = serde_json::from_str(
            r#"{"filename":"x.svg","content_type":"image/svg+xml","width":10,"height":10}"#,
        )
        .unwrap();
        assert!(!svg.is_image());
        let zip: Attachment = serde_json::from_str(r#"{"filename":"log.zip","size":5}"#).unwrap();
        assert!(!zip.is_image());
    }

    #[test]
    fn snowflake_ordinati_come_numeri() {
        assert_eq!(cmp_snowflake("99", "100"), Ordering::Less);
        assert_eq!(cmp_snowflake("200", "100"), Ordering::Greater);
    }

    #[test]
    fn ready_in_entrambi_i_formati() {
        let json = r#"{
            "user": {"id": "1", "username": "tommaso", "global_name": null},
            "session_id": "abc",
            "resume_gateway_url": "wss://gateway-us-east1-b.discord.gg",
            "guilds": [
                {"id": "10", "name": "Classico", "channels": [{"id": "11", "type": 0, "name": "general"}]},
                {"id": "20", "properties": {"name": "Con capabilities"}, "channels": []},
                {"id": "30", "unavailable": true}
            ],
            "private_channels": [
                {"id": "40", "type": 1, "recipient_ids": ["2"], "last_message_id": "500"}
            ],
            "users": [{"id": "2", "username": "giulia", "global_name": "Giulia"}],
            "campo_nuovo_sconosciuto": {"x": 1}
        }"#;
        let r: Ready = serde_json::from_str(json).unwrap();
        assert_eq!(r.guilds[0].name(), "Classico");
        assert_eq!(r.guilds[1].name(), "Con capabilities");
        assert_eq!(r.guilds[2].name(), "…");
        assert_eq!(r.private_channels[0].recipient_ids, vec!["2"]);
        assert_eq!(r.users[0].display_name(), "Giulia");
        assert_eq!(r.user.display_name(), "tommaso");
    }
}

/// Risultato della ricerca GIF di Discord (`/gifs/search`, `/gifs/trending-gifs`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GifResult {
    /// Pagina della GIF: è il link che si invia nel messaggio.
    pub url: String,
    /// File nel formato chiesto (`tinygif`: GIF piccola per la griglia).
    pub src: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
}
