//! Dati finti per `--demo`: servono a lavorare sull'interfaccia senza
//! connettersi a Discord (e senza rischi per l'account).

use chrono::{Duration, Local};
use serde_json::json;

use crate::discord::models::{Embed, GifResult, Message, Ready, User};

pub const UNREAD_CHANNEL: &str = "1003";
pub const DEMO_IMAGE_URL: &str = "demo://anteprima-gui.png";
/// GIF animata della demo: anche questa è generata dalla GUI, senza rete.
pub const DEMO_GIF_URL: &str = "demo://ferris.gif";
const ME: &str = "900";

pub fn ready() -> Ready {
    serde_json::from_value(json!({
        "user": {"id": ME, "username": "tommaso", "global_name": "Tommaso"},
        "session_id": "demo",
        "guilds": [
            {
                "id": "100", "name": "Rust Italia",
                "channels": [
                    {"id": "1000", "type": 0, "name": "benvenuto", "position": 0},
                    {"id": "1010", "type": 4, "name": "Chiacchiere", "position": 0},
                    {"id": "1001", "type": 0, "name": "generale", "position": 0, "parent_id": "1010"},
                    {"id": "1003", "type": 0, "name": "aiuto", "position": 1, "parent_id": "1010"},
                    {"id": "1004", "type": 5, "name": "annunci", "position": 2, "parent_id": "1010"},
                    {"id": "1020", "type": 4, "name": "Vocali", "position": 1},
                    {"id": "1021", "type": 2, "name": "Lounge", "position": 0, "parent_id": "1020"}
                ]
            },
            {
                "id": "200", "name": "Homelab & Networking",
                "channels": [
                    {"id": "2001", "type": 0, "name": "rete", "position": 0},
                    {"id": "2002", "type": 0, "name": "storage", "position": 1}
                ]
            },
            {
                "id": "300", "properties": {"name": "Gaming"},
                "channels": [{"id": "3001", "type": 0, "name": "lfg", "position": 0}]
            }
        ],
        "private_channels": [
            {"id": "5001", "type": 1, "recipient_ids": ["901"], "last_message_id": "70010"},
            {"id": "5002", "type": 3, "name": "Progetto client", "recipient_ids": ["901", "902"], "last_message_id": "70001"}
        ],
        "users": [
            {"id": "901", "username": "giulia.dev", "global_name": "Giulia"},
            {"id": "902", "username": "marco_ops", "global_name": "Marco"}
        ]
    }))
    .expect("dati demo validi")
}

fn user(id: &str, username: &str, name: &str) -> User {
    serde_json::from_value(json!({"id": id, "username": username, "global_name": name})).unwrap()
}

fn msg(id: u64, channel: &str, author: &User, minutes_ago: i64, content: &str) -> Message {
    let ts = (Local::now() - Duration::minutes(minutes_ago)).to_rfc3339();
    serde_json::from_value(json!({
        "id": id.to_string(), "channel_id": channel, "content": content, "timestamp": ts,
        "author": {"id": author.id, "username": author.username, "global_name": author.global_name},
    }))
    .unwrap()
}

pub fn messages() -> Vec<(String, Vec<Message>)> {
    let giulia = user("901", "giulia.dev", "Giulia");
    let marco = user("902", "marco_ops", "Marco");
    let me = user(ME, "tommaso", "Tommaso");

    let mut generale = vec![
        msg(
            60001,
            "1001",
            &marco,
            26 * 60,
            "Qualcuno ha provato il nuovo kernel sul NAS?",
        ),
        msg(
            60002,
            "1001",
            &giulia,
            26 * 60 - 2,
            "Sì, nessun problema con ZFS finora",
        ),
        msg(
            60003,
            "1001",
            &marco,
            95,
            "Ragazzi, stasera manutenzione della VPN dalle 22 alle 23",
        ),
        msg(60004, "1001", &marco, 94, "Avviso anche in <#1004>"),
        msg(
            60005,
            "1001",
            &giulia,
            40,
            "<@900> ho visto il tuo client in Rust! Quanta RAM usa rispetto a quello ufficiale? \
             Sto cercando di capire se convenga anche sul portatile vecchio, quello da 4 GB.",
        ),
        msg(
            60006,
            "1001",
            &me,
            35,
            "Niente Chromium dietro, parte in un attimo 🙂 Poi misuro bene",
        ),
        msg(
            60007,
            "1001",
            &me,
            34,
            "Il prossimo passo è il rendering delle immagini",
        ),
        msg(60008, "1001", &giulia, 3, "Fantastico <:ferris:123456789>"),
    ];
    generale.sort_by(|a, b| a.id.cmp(&b.id));
    // Mention vera (serve a evidenziarla), risposta, allegato e reazioni.
    generale[4].mentions = vec![me.clone()];
    generale[5].referenced_message = Some(Box::new(
        serde_json::from_value(json!({
            "id": "60005", "content": generale[4].content.clone(),
            "author": {"id": &giulia.id, "username": &giulia.username, "global_name": &giulia.global_name}
        }))
        .unwrap(),
    ));
    // Nella demo l'immagine non si scarica: l'indirizzo `demo://` viene
    // riconosciuto dalla GUI, che la genera al volo.
    generale[6].attachments = serde_json::from_value(json!([{
        "id": "1", "filename": "anteprima-gui.png", "size": 245_760,
        "url": DEMO_IMAGE_URL, "content_type": "image/png", "width": 960, "height": 600
    }]))
    .unwrap();
    generale[7].reactions = serde_json::from_value(json!([
        {"count": 2, "me": true, "emoji": {"id": null, "name": "🦀"}},
        {"count": 1, "me": false, "emoji": {"id": null, "name": "🚀"}}
    ]))
    .unwrap();
    // Un link a una GIF: Discord lo trasforma in un embed `gifv`.
    let mut gif = msg(
        60009,
        "1001",
        &marco,
        2,
        "https://tenor.com/view/ferris-dance-demo",
    );
    gif.embeds = serde_json::from_value(json!([{
        "type": "gifv", "url": "https://tenor.com/view/ferris-dance-demo",
        "provider": {"name": "Tenor"},
        "thumbnail": {"url": DEMO_GIF_URL, "width": 320, "height": 200},
        "video": {"url": "demo://ferris.mp4", "width": 320, "height": 200}
    }]))
    .unwrap();
    generale.push(gif);

    let aiuto = vec![msg(
        61001,
        UNREAD_CHANNEL,
        &giulia,
        10,
        "Come si gestisce il resume del gateway dopo una disconnessione?",
    )];

    let dm = vec![
        msg(
            70001,
            "5001",
            &giulia,
            60,
            "Ciao! Hai pushato il branch del login?",
        ),
        msg(70010, "5001", &me, 58, "Sì, c'è anche il QR code"),
    ];

    vec![
        ("1001".to_string(), generale),
        (UNREAD_CHANNEL.to_string(), aiuto),
        ("5001".to_string(), dm),
    ]
}

const DEMO_GIF_PAGE: &str = "https://klipy.com/gifs/ferris-demo-";

/// GIF del selettore nella demo: tutte la stessa animazione locale.
pub fn gif_results(query: &str) -> Vec<GifResult> {
    let query = query.trim();
    let count = if query.is_empty() { 6 } else { 3 };
    (1..=count)
        .map(|i| GifResult {
            url: format!("{DEMO_GIF_PAGE}{i}"),
            src: DEMO_GIF_URL.to_string(),
            title: format!("Ferris {i}"),
            width: 320,
            height: 200,
        })
        .collect()
}

/// Embed che Discord aggiungerebbe al link di una GIF della demo.
pub fn gif_embed(content: &str) -> Option<Embed> {
    if !content.starts_with(DEMO_GIF_PAGE) {
        return None;
    }
    serde_json::from_value(json!({
        "type": "gifv", "url": content, "provider": {"name": "Klipy"},
        "thumbnail": {"url": DEMO_GIF_URL, "width": 320, "height": 200},
        "video": {"url": "demo://ferris.mp4", "width": 320, "height": 200}
    }))
    .ok()
}

/// Messaggio creato localmente quando si invia qualcosa in modalità demo.
pub fn local_message(
    channel_id: &str,
    author: Option<User>,
    content: &str,
    existing: Option<&Vec<Message>>,
) -> Message {
    let next_id = existing
        .and_then(|v| v.last())
        .and_then(|m| m.id.parse::<u64>().ok())
        .unwrap_or(80_000)
        + 1;
    let author = author.unwrap_or_else(|| user("902", "marco_ops", "Marco"));
    msg(next_id, channel_id, &author, 0, content)
}
