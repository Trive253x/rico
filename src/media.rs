//! Download delle anteprime delle immagini allegate.
//!
//! Regole di sicurezza: si scarica solo via HTTPS e solo dai server di Discord
//! (CDN e media proxy), con un limite sulla dimensione della risposta; si
//! chiede al media proxy una miniatura già ridimensionata, così non si
//! scaricano né si decodificano immagini enormi.

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::discord::models::Attachment;

/// Lato massimo dell'anteprima mostrata nella chat (pixel logici).
pub const PREVIEW_MAX_W: u32 = 400;
pub const PREVIEW_MAX_H: u32 = 300;
/// Limite alla dimensione scaricata di una singola anteprima.
const MAX_BYTES: usize = 8 * 1024 * 1024;
const ALLOWED_HOSTS: [&str; 2] = ["cdn.discordapp.com", "media.discordapp.net"];

/// Dimensioni a cui mostrare l'immagine: rispettano le proporzioni e non
/// superano il riquadro dell'anteprima (né l'originale).
pub fn display_size(width: u32, height: u32) -> (f32, f32) {
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    let scale = (PREVIEW_MAX_W as f32 / w).min(PREVIEW_MAX_H as f32 / h).min(1.0);
    ((w * scale).round().max(1.0), (h * scale).round().max(1.0))
}

/// Indirizzo da scaricare: la miniatura del media proxy quando c'è, altrimenti l'originale.
/// Si chiede il doppio della dimensione mostrata, per gli schermi ad alta densità.
pub fn preview_url(a: &Attachment) -> Option<String> {
    let (Some(w), Some(h)) = (a.width, a.height) else {
        return None;
    };
    let base = if !a.proxy_url.is_empty() { &a.proxy_url } else { &a.url };
    if base.is_empty() {
        return None;
    }
    let (dw, dh) = display_size(w, h);
    let (tw, th) = ((dw * 2.0) as u32, (dh * 2.0) as u32);
    if base.contains("media.discordapp.net") && (tw < w || th < h) {
        let sep = if base.contains('?') { '&' } else { '?' };
        Some(format!("{base}{sep}width={tw}&height={th}"))
    } else {
        Some(base.clone())
    }
}

/// Controlla che l'indirizzo sia HTTPS e punti a un server di Discord.
pub fn check_url(url: &str) -> Result<()> {
    let Some(rest) = url.strip_prefix("https://") else {
        bail!("solo indirizzi https");
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        bail!("indirizzo non valido");
    }
    let host = authority.split(':').next().unwrap_or_default().to_ascii_lowercase();
    if !ALLOWED_HOSTS.contains(&host.as_str()) {
        bail!("server non consentito: {host}");
    }
    Ok(())
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(crate::discord::USER_AGENT)
            .timeout(Duration::from_secs(30))
            // I redirect potrebbero portare fuori dai server consentiti.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("client HTTP")
    })
}

/// Scarica i byte di un'anteprima rispettando le regole sopra.
pub async fn fetch(url: &str) -> Result<Vec<u8>> {
    check_url(url)?;
    let mut resp = client().get(url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        bail!("HTTP {}", status.as_u16());
    }
    if let Some(len) = resp.content_length() {
        if len as usize > MAX_BYTES {
            bail!("immagine troppo grande ({len} byte)");
        }
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if !content_type.is_empty() && !content_type.starts_with("image/") {
        bail!("il server non ha restituito un'immagine ({content_type})");
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.context("download interrotto")? {
        if body.len() + chunk.len() > MAX_BYTES {
            bail!("immagine troppo grande");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Immagine finta per la modalità demo (nessuna connessione): sfumatura con
/// una griglia, in RGBA.
pub fn demo_pixels(width: u32, height: u32) -> Vec<u8> {
    let mut px = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let grid = x % 40 == 0 || y % 40 == 0;
            let r = (40 + 80 * x / width.max(1)) as u8;
            let g = (60 + 100 * y / height.max(1)) as u8;
            let b = 160u8;
            if grid {
                px.extend_from_slice(&[r / 2, g / 2, b / 2, 255]);
            } else {
                px.extend_from_slice(&[r, g, b, 255]);
            }
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(json: &str) -> Attachment {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn dimensioni_proporzionate() {
        assert_eq!(display_size(800, 600), (400.0, 300.0));
        assert_eq!(display_size(1000, 200), (400.0, 80.0));
        assert_eq!(display_size(100, 50), (100.0, 50.0));
        assert_eq!(display_size(0, 0), (1.0, 1.0));
    }

    #[test]
    fn miniatura_dal_media_proxy() {
        let a = att(
            r#"{"url":"https://cdn.discordapp.com/attachments/1/2/a.png?ex=1&hm=2","proxy_url":"https://media.discordapp.net/attachments/1/2/a.png?ex=1&hm=2","width":4000,"height":3000}"#,
        );
        assert_eq!(
            preview_url(&a).unwrap(),
            "https://media.discordapp.net/attachments/1/2/a.png?ex=1&hm=2&width=800&height=600"
        );
        // Immagine già piccola: niente ridimensionamento.
        let small = att(r#"{"proxy_url":"https://media.discordapp.net/a.png","width":100,"height":80}"#);
        assert_eq!(preview_url(&small).unwrap(), "https://media.discordapp.net/a.png");
        // Senza dimensioni non è un'immagine da mostrare.
        assert!(preview_url(&att(r#"{"url":"https://cdn.discordapp.com/x.png"}"#)).is_none());
    }

    #[test]
    fn solo_server_di_discord() {
        assert!(check_url("https://cdn.discordapp.com/attachments/1/2/a.png").is_ok());
        assert!(check_url("https://media.discordapp.net/a.png?width=10").is_ok());
        assert!(check_url("http://cdn.discordapp.com/a.png").is_err());
        assert!(check_url("https://evil.example/a.png").is_err());
        assert!(check_url("https://cdn.discordapp.com.evil.example/a.png").is_err());
        assert!(check_url("https://user@evil.example/cdn.discordapp.com").is_err());
        assert!(check_url("https://cdn.discordapp.com@evil.example/a.png").is_err());
    }

    /// Scarica un avatar predefinito pubblico di Discord. `cargo test -- --ignored`
    #[tokio::test]
    #[ignore = "richiede la rete"]
    async fn scarica_immagine_dalla_cdn() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let bytes = fetch("https://cdn.discordapp.com/embed/avatars/0.png").await.unwrap();
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']), "non è un PNG");
    }
}
