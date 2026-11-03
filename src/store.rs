//! Dati della sessione, condivisi tra GUI e terminale: server, canali,
//! conversazioni private, messaggi caricati, non letti, chi sta scrivendo.
//!
//! Qui c'è solo logica sui dati: nessuna rete e nessuna interfaccia. Le due
//! interfacce chiamano questi metodi quando arrivano eventi dal gateway o
//! risposte dalle API REST.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::discord::models::{
    cmp_snowflake, kind, Channel, Guild, GuildEmoji, GuildEmojisUpdate, Message, MessageDelete,
    MessageUpdate, Reaction, ReactionEvent, Ready, TypingStart, User, VoiceState,
};
use crate::discord::rest::HISTORY_PAGE;

/// Per quanto resta visibile "sta scrivendo…" dopo l'ultimo TYPING_START.
pub const TYPING_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelRow {
    Category(String),
    Channel {
        id: String,
        name: String,
        kind: u8,
        selectable: bool,
    },
}

impl ChannelRow {
    pub fn is_selectable(&self) -> bool {
        matches!(self, ChannelRow::Channel { selectable: true, .. })
    }
}

#[derive(Default)]
pub struct Store {
    pub me: Option<User>,
    pub guilds: Vec<Guild>,
    pub dms: Vec<Channel>,
    pub users: HashMap<String, User>,
    pub channel_names: HashMap<String, String>,
    pub channel_guild: HashMap<String, String>,
    /// Messaggi caricati per canale, in ordine cronologico.
    pub messages: HashMap<String, Vec<Message>>,
    pub loading: HashSet<String>,
    /// Canali di cui abbiamo già tutta la cronologia.
    pub exhausted: HashSet<String>,
    pub unread: HashSet<String>,
    /// Canale aperto.
    pub current: Option<String>,
    /// Chi sta scrivendo, per canale: (id utente, nome, quando).
    typing: HashMap<String, Vec<(String, String, Instant)>>,
    /// Chi è in un canale vocale: utente → (server, canale).
    pub voice: HashMap<String, (String, String)>,
}

impl Store {
    /// Dati finti per la modalità demo.
    pub fn demo() -> Self {
        let mut store = Store::default();
        store.apply_ready(crate::demo::ready());
        for (channel_id, msgs) in crate::demo::messages() {
            for m in &msgs {
                store.remember_users(m);
            }
            store.exhausted.insert(channel_id.clone());
            store.messages.insert(channel_id, msgs);
        }
        store.unread.insert(crate::demo::UNREAD_CHANNEL.to_string());
        store
    }

    pub fn me_id(&self) -> Option<&str> {
        self.me.as_ref().map(|u| u.id.as_str())
    }

    /// Nuova sessione (READY). Restituisce il canale aperto, da ricaricare:
    /// dopo un READY (non un RESUME) possono mancare dei messaggi.
    pub fn apply_ready(&mut self, r: Ready) -> Option<String> {
        for u in r.users {
            self.users.insert(u.id.clone(), u);
        }
        self.users.insert(r.user.id.clone(), r.user.clone());
        self.me = Some(r.user);

        self.guilds.clear();
        self.channel_guild.clear();
        self.channel_names.clear();
        self.voice.clear();
        for g in r.guilds {
            self.upsert_guild(g);
        }

        let mut dms = r.private_channels;
        for c in &dms {
            for u in &c.recipients {
                self.users.insert(u.id.clone(), u.clone());
            }
        }
        dms.sort_by(|a, b| {
            cmp_snowflake(
                b.last_message_id.as_deref().unwrap_or("0"),
                a.last_message_id.as_deref().unwrap_or("0"),
            )
        });
        self.dms = dms;

        self.messages.clear();
        self.exhausted.clear();
        self.loading.clear();
        self.typing.clear();
        self.current.clone()
    }

    pub fn upsert_guild(&mut self, mut g: Guild) {
        self.voice.retain(|_, (guild, _)| *guild != g.id);
        for v in std::mem::take(&mut g.voice_states) {
            self.apply_voice_state(&v, Some(&g.id));
        }
        for c in &mut g.channels {
            c.guild_id = Some(g.id.clone());
            self.channel_guild.insert(c.id.clone(), g.id.clone());
            if let Some(n) = &c.name {
                self.channel_names.insert(c.id.clone(), n.clone());
            }
        }
        match self.guilds.iter_mut().find(|x| x.id == g.id) {
            Some(slot) => *slot = g,
            None => self.guilds.push(g),
        }
    }

    /// Qualcuno è entrato, uscito o passato a un altro canale vocale.
    pub fn apply_voice_state(&mut self, v: &VoiceState, guild_id: Option<&str>) {
        if let Some(user) = v.member.as_ref().and_then(|m| m.user.clone()) {
            self.users.entry(user.id.clone()).or_insert(user);
        }
        match (v.guild_id.as_deref().or(guild_id), &v.channel_id) {
            (Some(guild), Some(channel)) => {
                self.voice
                    .insert(v.user_id.clone(), (guild.to_string(), channel.clone()));
            }
            _ => {
                self.voice.remove(&v.user_id);
            }
        }
    }

    /// Chi è nel canale vocale, in ordine di nome: (id, nome).
    pub fn voice_members(&self, channel_id: &str) -> Vec<(String, String)> {
        let mut members: Vec<(String, String)> = self
            .voice
            .iter()
            .filter(|(_, (_, c))| c == channel_id)
            .map(|(id, _)| {
                let name = self
                    .users
                    .get(id)
                    .map(|u| u.display_name().to_string())
                    .unwrap_or_else(|| "utente".into());
                (id.clone(), name)
            })
            .collect();
        members.sort_by_key(|(_, name)| name.to_lowercase());
        members
    }

    pub fn remember_users(&mut self, m: &Message) {
        self.users
            .entry(m.author.id.clone())
            .or_insert_with(|| m.author.clone());
        for u in &m.mentions {
            self.users.entry(u.id.clone()).or_insert_with(|| u.clone());
        }
    }

    /// Apre un canale; restituisce `true` se va scaricata la cronologia.
    pub fn open(&mut self, channel_id: &str) -> bool {
        self.unread.remove(channel_id);
        self.current = Some(channel_id.to_string());
        !self.exhausted.contains(channel_id) && self.messages.get(channel_id).is_none_or(|v| v.is_empty())
    }

    /// Segna l'inizio di un caricamento; `false` se ce n'è già uno in corso.
    pub fn begin_loading(&mut self, channel_id: &str) -> bool {
        self.loading.insert(channel_id.to_string())
    }

    /// ID del messaggio più vecchio caricato, se c'è altra cronologia da scaricare.
    pub fn older_cursor(&self, channel_id: &str) -> Option<String> {
        if self.exhausted.contains(channel_id) || self.loading.contains(channel_id) {
            return None;
        }
        self.messages
            .get(channel_id)
            .and_then(|v| v.first())
            .map(|m| m.id.clone())
    }

    /// Applica una pagina di cronologia (arriva dal più recente al più vecchio).
    pub fn apply_history(&mut self, channel_id: &str, older: bool, mut batch: Vec<Message>) {
        self.loading.remove(channel_id);
        if batch.len() < HISTORY_PAGE {
            self.exhausted.insert(channel_id.to_string());
        }
        for m in &batch {
            self.remember_users(m);
        }
        batch.reverse();
        let list = self.messages.entry(channel_id.to_string()).or_default();
        if older {
            batch.retain(|m| !list.iter().any(|x| x.id == m.id));
            batch.append(list);
        } else {
            // Unisce eventuali messaggi arrivati dal gateway durante il caricamento.
            for m in list.drain(..) {
                if !batch.iter().any(|x| x.id == m.id) {
                    batch.push(m);
                }
            }
            batch.sort_by(|a, b| cmp_snowflake(&a.id, &b.id));
        }
        *list = batch;
    }

    pub fn history_failed(&mut self, channel_id: &str) {
        self.loading.remove(channel_id);
    }

    /// Inserisce un messaggio (dal gateway o dalla risposta all'invio), senza duplicati.
    /// Restituisce `true` se è finito nel canale aperto.
    pub fn insert_message(&mut self, m: Message) -> bool {
        self.remember_users(&m);
        let from_me = self.me_id() == Some(m.author.id.as_str());
        let in_current = self.current.as_deref() == Some(m.channel_id.as_str());
        if !in_current && !from_me {
            self.unread.insert(m.channel_id.clone());
        }
        if let Some(list) = self.typing.get_mut(&m.channel_id) {
            list.retain(|(uid, _, _)| uid != &m.author.id);
        }
        self.bump_dm(&m.channel_id, &m.id);

        // Se il canale non è mai stato aperto non conserviamo nulla: all'apertura
        // si scarica comunque la cronologia completa.
        let Some(list) = self.messages.get_mut(&m.channel_id) else {
            return false;
        };
        if list.iter().rev().take(100).any(|x| x.id == m.id) {
            return false;
        }
        let out_of_order = list.last().is_some_and(|l| cmp_snowflake(&l.id, &m.id).is_gt());
        list.push(m);
        if out_of_order {
            list.sort_by(|a, b| cmp_snowflake(&a.id, &b.id));
        }
        in_current
    }

    pub fn update_message(&mut self, u: MessageUpdate) {
        let Some(m) = self.find_message_mut(&u.channel_id, &u.id) else {
            return;
        };
        if let Some(c) = u.content {
            m.content = c;
        }
        if u.edited_timestamp.is_some() {
            m.edited_timestamp = u.edited_timestamp;
        }
        if let Some(a) = u.attachments {
            m.attachments = a;
        }
        if let Some(e) = u.embeds {
            m.embeds = e;
        }
    }

    pub fn delete_message(&mut self, d: &MessageDelete) {
        if let Some(list) = self.messages.get_mut(&d.channel_id) {
            list.retain(|m| m.id != d.id);
        }
    }

    fn find_message_mut(&mut self, channel_id: &str, message_id: &str) -> Option<&mut Message> {
        self.messages
            .get_mut(channel_id)
            .and_then(|l| l.iter_mut().rev().find(|m| m.id == message_id))
    }

    pub fn reaction_add(&mut self, ev: &ReactionEvent) {
        let mine = self.me_id() == Some(ev.user_id.as_str());
        let Some(m) = self.find_message_mut(&ev.channel_id, &ev.message_id) else {
            return;
        };
        match m.reactions.iter_mut().find(|r| r.emoji.same(&ev.emoji)) {
            Some(r) => {
                r.count += 1;
                r.me |= mine;
            }
            None => m.reactions.push(Reaction {
                count: 1,
                me: mine,
                emoji: ev.emoji.clone(),
            }),
        }
    }

    pub fn reaction_remove(&mut self, ev: &ReactionEvent) {
        let mine = self.me_id() == Some(ev.user_id.as_str());
        let Some(m) = self.find_message_mut(&ev.channel_id, &ev.message_id) else {
            return;
        };
        if let Some(r) = m.reactions.iter_mut().find(|r| r.emoji.same(&ev.emoji)) {
            r.count = r.count.saturating_sub(1);
            if mine {
                r.me = false;
            }
        }
        m.reactions.retain(|r| r.count > 0);
    }

    pub fn typing_start(&mut self, t: TypingStart) {
        if self.me_id() == Some(t.user_id.as_str()) {
            return;
        }
        let name = t
            .member
            .as_ref()
            .and_then(|m| {
                m.nick
                    .clone()
                    .or_else(|| m.user.as_ref().map(|u| u.display_name().to_string()))
            })
            .or_else(|| self.users.get(&t.user_id).map(|u| u.display_name().to_string()))
            .unwrap_or_else(|| "Qualcuno".to_string());
        let list = self.typing.entry(t.channel_id).or_default();
        list.retain(|(uid, _, _)| uid != &t.user_id);
        list.push((t.user_id, name, Instant::now()));
    }

    /// Nomi di chi sta scrivendo nel canale (scadono dopo `TYPING_TIMEOUT`).
    pub fn typing_in(&self, channel_id: &str) -> Vec<&str> {
        self.typing
            .get(channel_id)
            .map(|l| {
                l.iter()
                    .filter(|(_, _, at)| at.elapsed() < TYPING_TIMEOUT)
                    .map(|(_, name, _)| name.as_str())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn has_typing(&self) -> bool {
        self.typing
            .values()
            .any(|l| l.iter().any(|(_, _, at)| at.elapsed() < TYPING_TIMEOUT))
    }

    /// Simula "sta scrivendo…" (usato dalla demo).
    pub fn fake_typing(&mut self, channel_id: &str, user_id: &str, name: &str) {
        self.typing.entry(channel_id.to_string()).or_default().push((
            user_id.to_string(),
            name.to_string(),
            Instant::now(),
        ));
    }

    /// Porta in cima la conversazione privata che ha appena ricevuto un messaggio.
    fn bump_dm(&mut self, channel_id: &str, message_id: &str) {
        let Some(pos) = self.dms.iter().position(|c| c.id == channel_id) else {
            return;
        };
        self.dms[pos].last_message_id = Some(message_id.to_string());
        if pos > 0 {
            let c = self.dms.remove(pos);
            self.dms.insert(0, c);
        }
    }

    // ------------------------------------------------------------ viste sui dati

    /// Righe della colonna canali: `None` = messaggi diretti, altrimenti l'indice del server.
    pub fn channel_rows(&self, guild: Option<usize>) -> Vec<ChannelRow> {
        match guild.and_then(|i| self.guilds.get(i)) {
            None => self
                .dms
                .iter()
                .map(|c| ChannelRow::Channel {
                    id: c.id.clone(),
                    name: self.dm_name(c),
                    kind: c.kind,
                    selectable: true,
                })
                .collect(),
            Some(g) => guild_rows(&g.channels),
        }
    }

    pub fn dm_name(&self, c: &Channel) -> String {
        if let Some(name) = c.name.as_deref().filter(|n| !n.is_empty()) {
            return name.to_string();
        }
        let names: Vec<&str> = if c.recipients.is_empty() {
            c.recipient_ids
                .iter()
                .filter_map(|id| self.users.get(id))
                .map(User::display_name)
                .collect()
        } else {
            c.recipients.iter().map(User::display_name).collect()
        };
        if names.is_empty() {
            "Conversazione".into()
        } else {
            names.join(", ")
        }
    }

    pub fn is_dm(&self, channel_id: &str) -> bool {
        self.dms.iter().any(|c| c.id == channel_id)
    }

    /// Nome breve del canale: `#nome` oppure `@persona`.
    pub fn channel_label(&self, channel_id: &str) -> String {
        if let Some(dm) = self.dms.iter().find(|c| c.id == channel_id) {
            return format!("@{}", self.dm_name(dm));
        }
        let name = self
            .channel_names
            .get(channel_id)
            .map(String::as_str)
            .unwrap_or("canale");
        format!("#{name}")
    }

    pub fn guild_emojis(&mut self, u: GuildEmojisUpdate) {
        if let Some(g) = self.guilds.iter_mut().find(|g| g.id == u.guild_id) {
            g.emojis = u.emojis;
        }
    }

    /// Emoji del server che si possono usare nel canale: solo quelle del
    /// server a cui appartiene (nei messaggi diretti nessuna), disponibili,
    /// senza limiti di ruolo, e animate solo con Nitro.
    pub fn usable_emojis(&self, channel_id: &str) -> Vec<&GuildEmoji> {
        let nitro = self.me.as_ref().and_then(|u| u.premium_type).unwrap_or(0) > 0;
        self.guild_of(channel_id)
            .map(|g| {
                g.emojis
                    .iter()
                    .filter(|e| e.available && e.roles.is_empty() && (nitro || !e.animated))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Testo da inviare: `:nome:` diventa l'emoji del server del canale.
    pub fn prepare_outgoing(&self, channel_id: &str, content: &str) -> String {
        let usable: Vec<GuildEmoji> = self.usable_emojis(channel_id).into_iter().cloned().collect();
        crate::text::expand_custom_emojis(content, &usable)
    }

    pub fn guild_of(&self, channel_id: &str) -> Option<&Guild> {
        self.channel_guild
            .get(channel_id)
            .and_then(|gid| self.guilds.iter().find(|g| &g.id == gid))
    }

    /// Titolo del canale: `#nome · server` oppure `@persona`.
    pub fn title(&self, channel_id: &str) -> String {
        let label = self.channel_label(channel_id);
        match self.guild_of(channel_id) {
            Some(g) => format!("{label} · {}", g.name()),
            None => label,
        }
    }

    pub fn guild_has_unread(&self, guild_id: &str) -> bool {
        self.unread
            .iter()
            .any(|c| self.channel_guild.get(c).is_some_and(|g| g == guild_id))
    }

    pub fn dms_have_unread(&self) -> bool {
        self.dms.iter().any(|c| self.unread.contains(&c.id))
    }
}

pub fn first_selectable(rows: &[ChannelRow]) -> usize {
    rows.iter().position(ChannelRow::is_selectable).unwrap_or(0)
}

/// Ordina i canali come Discord: prima quelli senza categoria, poi ogni
/// categoria con i suoi canali (testo prima della voce, poi per posizione).
pub fn guild_rows(channels: &[Channel]) -> Vec<ChannelRow> {
    let visible = |c: &&Channel| c.is_text() || c.is_voice();
    let order = |a: &&Channel, b: &&Channel| {
        (a.is_voice(), a.position.unwrap_or(0))
            .cmp(&(b.is_voice(), b.position.unwrap_or(0)))
            .then_with(|| cmp_snowflake(&a.id, &b.id))
    };
    let row = |c: &Channel| ChannelRow::Channel {
        id: c.id.clone(),
        name: c.name.clone().unwrap_or_default(),
        kind: c.kind,
        selectable: c.is_text(),
    };

    let mut rows = Vec::new();
    let mut top: Vec<&Channel> = channels
        .iter()
        .filter(|c| c.parent_id.is_none())
        .filter(visible)
        .collect();
    top.sort_by(order);
    rows.extend(top.into_iter().map(row));

    let mut categories: Vec<&Channel> = channels.iter().filter(|c| c.kind == kind::CATEGORY).collect();
    categories.sort_by(|a, b| {
        a.position
            .unwrap_or(0)
            .cmp(&b.position.unwrap_or(0))
            .then_with(|| cmp_snowflake(&a.id, &b.id))
    });
    for cat in categories {
        let mut children: Vec<&Channel> = channels
            .iter()
            .filter(|c| c.parent_id.as_deref() == Some(cat.id.as_str()))
            .filter(visible)
            .collect();
        if children.is_empty() {
            continue;
        }
        children.sort_by(order);
        rows.push(ChannelRow::Category(cat.name.clone().unwrap_or_default()));
        rows.extend(children.into_iter().map(row));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::models::Emoji;

    #[test]
    fn chi_e_nei_canali_vocali() {
        let mut store = Store::demo();
        let names: Vec<String> = store.voice_members("1021").into_iter().map(|(_, n)| n).collect();
        assert_eq!(names, ["Giulia", "Marco"]);
        let state = |v: serde_json::Value| -> VoiceState { serde_json::from_value(v).unwrap() };
        // Giulia esce, arriva qualcuno che non conoscevamo.
        store.apply_voice_state(
            &state(serde_json::json!({"guild_id": "100", "channel_id": null, "user_id": "901"})),
            None,
        );
        store.apply_voice_state(
            &state(serde_json::json!({"guild_id": "100", "channel_id": "1021", "user_id": "950",
                "member": {"user": {"id": "950", "username": "nuovo"}}})),
            None,
        );
        let names: Vec<String> = store.voice_members("1021").into_iter().map(|(_, n)| n).collect();
        assert_eq!(names, ["Marco", "nuovo"]);
        assert!(store.voice_members("1003").is_empty());
    }

    #[test]
    fn canali_ordinati_per_categoria() {
        let channels: Vec<Channel> = serde_json::from_value(serde_json::json!([
            {"id": "5", "type": 2, "name": "Voce", "position": 0, "parent_id": "1"},
            {"id": "4", "type": 0, "name": "off-topic", "position": 1, "parent_id": "1"},
            {"id": "3", "type": 0, "name": "generale", "position": 0, "parent_id": "1"},
            {"id": "1", "type": 4, "name": "Testo", "position": 0},
            {"id": "2", "type": 0, "name": "regole", "position": 0},
            {"id": "9", "type": 4, "name": "Vuota", "position": 1},
        ]))
        .unwrap();
        let rows = guild_rows(&channels);
        let names: Vec<String> = rows
            .iter()
            .map(|r| match r {
                ChannelRow::Category(n) => format!("[{n}]"),
                ChannelRow::Channel { name, .. } => name.clone(),
            })
            .collect();
        assert_eq!(names, ["regole", "[Testo]", "generale", "off-topic", "Voce"]);
        assert_eq!(first_selectable(&rows), 0);
        assert!(!rows[4].is_selectable());
    }

    #[test]
    fn messaggi_senza_duplicati_e_non_letti() {
        let mut store = Store::demo();
        let channel = crate::demo::UNREAD_CHANNEL.to_string();
        let before = store.messages[&channel].len();
        let msg = crate::demo::local_message(&channel, None, "ciao", store.messages.get(&channel));
        assert!(!store.insert_message(msg.clone()));
        store.insert_message(msg);
        assert_eq!(store.messages[&channel].len(), before + 1);
        assert!(store.unread.contains(&channel));
        assert!(!store.open(&channel));
        assert!(!store.unread.contains(&channel));
    }

    #[test]
    fn reazioni_dal_gateway() {
        let mut store = Store::demo();
        let channel = crate::demo::UNREAD_CHANNEL.to_string();
        let id = store.messages[&channel][0].id.clone();
        let ev = |user: &str| ReactionEvent {
            user_id: user.into(),
            channel_id: channel.clone(),
            message_id: id.clone(),
            emoji: Emoji {
                id: None,
                name: Some("👍".into()),
                animated: false,
            },
        };
        store.reaction_add(&ev("901"));
        store.reaction_add(&ev("900"));
        let r = &store.messages[&channel][0].reactions[0];
        assert_eq!((r.count, r.me), (2, true));
        store.reaction_remove(&ev("900"));
        store.reaction_remove(&ev("901"));
        assert!(store.messages[&channel][0].reactions.is_empty());
    }

    #[test]
    fn dm_risale_in_cima() {
        let mut store = Store::demo();
        let last = store.dms.last().unwrap().id.clone();
        let msg = crate::demo::local_message(&last, None, "ehi", None);
        store.insert_message(msg);
        assert_eq!(store.dms[0].id, last);
    }
}
