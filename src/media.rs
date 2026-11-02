//! Download delle anteprime delle immagini allegate.
//!
//! Regole di sicurezza: si scarica solo via HTTPS e solo dai server di Discord
//! (CDN e media proxy), con un limite sulla dimensione della risposta; si
//! chiede al media proxy una miniatura già ridimensionata, così non si
//! scaricano né si decodificano immagini enormi.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::discord::models::Attachment;

/// Lato massimo dell'anteprima mostrata nella chat (pixel logici).
pub const PREVIEW_MAX_W: u32 = 400;
pub const PREVIEW_MAX_H: u32 = 300;
/// Limite alla dimensione scaricata di una singola anteprima.
pub const PREVIEW_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Limite per l'immagine a schermo intero (versione ridimensionata dal proxy).
pub const VIEWER_MAX_BYTES: usize = 25 * 1024 * 1024;
/// Lato massimo dell'immagine richiesta per la vista a schermo intero.
const VIEWER_MAX_SIDE: u32 = 2560;
/// Limite per il salvataggio dell'originale (gli allegati di Discord arrivano a 500 MB).
const FILE_MAX_BYTES: u64 = 600 * 1024 * 1024;
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
    let base = if !a.proxy_url.is_empty() {
        &a.proxy_url
    } else {
        &a.url
    };
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

/// Indirizzo per la vista a schermo intero: il media proxy ridimensiona le
/// immagini più grandi di `VIEWER_MAX_SIDE`, le altre arrivano intere.
pub fn viewer_url(a: &Attachment) -> Option<String> {
    let (Some(w), Some(h)) = (a.width, a.height) else {
        return None;
    };
    let base = if !a.proxy_url.is_empty() {
        &a.proxy_url
    } else {
        &a.url
    };
    if base.is_empty() {
        return None;
    }
    let scale = (VIEWER_MAX_SIDE as f32 / w.max(1) as f32)
        .min(VIEWER_MAX_SIDE as f32 / h.max(1) as f32)
        .min(1.0);
    if base.contains("media.discordapp.net") && scale < 1.0 {
        let (tw, th) = ((w as f32 * scale) as u32, (h as f32 * scale) as u32);
        let sep = if base.contains('?') { '&' } else { '?' };
        Some(format!("{base}{sep}width={}&height={}", tw.max(1), th.max(1)))
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
    let host = authority
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
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

/// Client per salvare file grandi: niente timeout complessivo, ma un limite
/// sull'attesa tra un pezzo e l'altro.
fn file_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(crate::discord::USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("client HTTP")
    })
}

/// Scarica i byte di un'immagine (al massimo `max_bytes`) rispettando le regole sopra.
pub async fn fetch(url: &str, max_bytes: usize) -> Result<Vec<u8>> {
    check_url(url)?;
    let mut resp = client().get(url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        bail!("HTTP {}", status.as_u16());
    }
    if let Some(len) = resp.content_length() {
        if len as usize > max_bytes {
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
        if body.len() + chunk.len() > max_bytes {
            bail!("immagine troppo grande");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Nome di file sicuro a partire da quello scelto da chi ha caricato l'allegato:
/// niente percorsi, caratteri vietati su Windows, nomi riservati o spazi e
/// punti in fondo; lunghezza limitata, estensione conservata.
pub fn safe_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut out: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    out = out.trim().trim_end_matches(['.', ' ']).to_string();
    while out.starts_with('.') {
        out.remove(0);
    }
    if out.is_empty() {
        out = "immagine".to_string();
    }
    let (stem, ext) = match out.rfind('.') {
        Some(i) if i > 0 && out.len() - i <= 10 => (out[..i].to_string(), out[i..].to_string()),
        _ => (out.clone(), String::new()),
    };
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
        "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let mut stem = if RESERVED.contains(&stem.to_ascii_uppercase().as_str()) {
        format!("_{stem}")
    } else {
        stem
    };
    let max_stem = 120usize.saturating_sub(ext.chars().count());
    if stem.chars().count() > max_stem {
        stem = stem.chars().take(max_stem).collect();
    }
    format!("{stem}{ext}")
}

/// Primo percorso libero nella cartella: `foto.jpg`, `foto (1).jpg`, `foto (2).jpg`…
pub fn unique_path(dir: &Path, filename: &str) -> PathBuf {
    let first = dir.join(filename);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match filename.rfind('.') {
        Some(i) if i > 0 => (&filename[..i], &filename[i..]),
        _ => (filename, ""),
    };
    (1..10_000)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// Cartella in cui salvare i file scaricati: Download dell'utente, se esiste.
pub fn download_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|d| {
            d.download_dir()
                .map(Path::to_path_buf)
                .or_else(|| Some(d.home_dir().to_path_buf()))
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Salva l'allegato originale in `dir`. Scrive prima un file `.part` e lo
/// rinomina solo a download completo: un file a metà non resta col nome giusto.
pub async fn save_to(url: &str, dir: &Path, filename: &str) -> Result<PathBuf> {
    use tokio::io::AsyncWriteExt;

    check_url(url)?;
    let mut resp = file_client().get(url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        bail!("HTTP {}", status.as_u16());
    }
    if resp.content_length().is_some_and(|len| len > FILE_MAX_BYTES) {
        bail!("file troppo grande");
    }
    tokio::fs::create_dir_all(dir).await?;
    let name = safe_filename(filename);
    let part = unique_path(dir, &format!("{name}.part"));
    let result = async {
        let mut file = tokio::fs::File::create(&part).await?;
        let mut written: u64 = 0;
        while let Some(chunk) = resp.chunk().await.context("download interrotto")? {
            written += chunk.len() as u64;
            if written > FILE_MAX_BYTES {
                bail!("file troppo grande");
            }
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        drop(file);
        // Il nome definitivo si sceglie a download finito: nel frattempo
        // potrebbe essere comparso un file con lo stesso nome.
        let target = unique_path(dir, &name);
        tokio::fs::rename(&part, &target).await?;
        Ok(target)
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
    }
    result
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
    fn immagine_grande_per_la_vista_intera() {
        let big =
            att(r#"{"proxy_url":"https://media.discordapp.net/a.png?ex=1","width":5120,"height":2880}"#);
        assert_eq!(
            viewer_url(&big).unwrap(),
            "https://media.discordapp.net/a.png?ex=1&width=2560&height=1440"
        );
        let small = att(r#"{"proxy_url":"https://media.discordapp.net/a.png","width":800,"height":600}"#);
        assert_eq!(viewer_url(&small).unwrap(), "https://media.discordapp.net/a.png");
    }

    #[test]
    fn nomi_di_file_sicuri() {
        assert_eq!(safe_filename("foto.jpg"), "foto.jpg");
        assert_eq!(safe_filename("../../Windows/evil.exe"), "evil.exe");
        assert_eq!(safe_filename("..\\..\\x.png"), "x.png");
        assert_eq!(safe_filename("a<b>c:d?.png"), "a_b_c_d_.png");
        assert_eq!(safe_filename("CON.txt"), "_CON.txt");
        assert_eq!(safe_filename(".bashrc"), "bashrc");
        assert_eq!(safe_filename("nome. . "), "nome");
        assert_eq!(safe_filename(""), "immagine");
        assert_eq!(safe_filename("..."), "immagine");
        let long = format!("{}.png", "x".repeat(500));
        let safe = safe_filename(&long);
        assert!(safe.ends_with(".png") && safe.chars().count() == 120);
    }

    #[test]
    fn percorso_libero() {
        let dir = std::env::temp_dir().join(format!("ferrocord-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unique_path(&dir, "a.png"), dir.join("a.png"));
        std::fs::write(dir.join("a.png"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "a.png"), dir.join("a (1).png"));
        std::fs::write(dir.join("a (1).png"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "a.png"), dir.join("a (2).png"));
        std::fs::remove_dir_all(&dir).unwrap();
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
        let bytes = fetch(
            "https://cdn.discordapp.com/embed/avatars/0.png",
            PREVIEW_MAX_BYTES,
        )
        .await
        .unwrap();
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']), "non è un PNG");
    }

    /// Salva un avatar predefinito pubblico in una cartella temporanea.
    #[tokio::test]
    #[ignore = "richiede la rete"]
    async fn salva_file_dalla_cdn() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let dir = std::env::temp_dir().join(format!("ferrocord-save-{}", std::process::id()));
        let path = save_to(
            "https://cdn.discordapp.com/embed/avatars/1.png",
            &dir,
            "../avatar.png",
        )
        .await
        .unwrap();
        assert_eq!(path, dir.join("avatar.png"));
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
        assert!(!dir.join("avatar.png.part").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
