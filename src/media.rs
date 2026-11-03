//! Download delle anteprime delle immagini allegate e delle GIF dei link.
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
/// Server da cui si scaricano immagini: quelli di Discord (CDN, media proxy e
/// proxy delle immagini esterne) e i server delle GIF di Tenor e Giphy, che
/// Discord manda solo come video MP4 (vedi `gif_from_video`).
const ALLOWED_HOSTS: [&str; 14] = [
    "cdn.discordapp.com",
    "media.discordapp.net",
    "images-ext-1.discordapp.net",
    "images-ext-2.discordapp.net",
    "media.tenor.com",
    "media1.tenor.com",
    "media.giphy.com",
    "i.giphy.com",
    "media0.giphy.com",
    "media1.giphy.com",
    "media2.giphy.com",
    "media3.giphy.com",
    "media4.giphy.com",
    // GIF della ricerca di Discord (fornite da Klipy).
    "static.klipy.com",
];
/// Fotogrammi massimi di una GIF animata e memoria massima che possono occupare
/// una volta decodificati: oltre, l'animazione si ferma ai fotogrammi già letti.
const GIF_MAX_FRAMES: usize = 300;
const GIF_MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;
/// GIF più grandi di così (in pixel per lato) non vengono nemmeno decodificate.
const GIF_MAX_SIDE: u32 = 4096;

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

fn host_of(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    Some(authority.split(':').next()?.to_ascii_lowercase())
}

/// Indirizzo della GIF a partire dal video MP4 che Discord manda per i link di
/// Tenor e Giphy. Tenor usa lo stesso ID per tutti i formati e cambia solo il
/// codice finale (`AAAPo` = MP4, `AAAAd` = GIF media); Giphy cambia il nome del file.
pub fn gif_from_video(video_url: &str) -> Option<String> {
    let host = host_of(video_url)?;
    let path_end = video_url.find(['?', '#']).unwrap_or(video_url.len());
    let path = &video_url[..path_end];
    if host.ends_with("tenor.com") {
        let (dir, file) = path.rsplit_once('/')?;
        let (base, code) = dir.rsplit_once('/')?;
        let id = code.strip_suffix("AAAPo")?;
        let name = file.strip_suffix(".mp4")?;
        if id.is_empty() || name.is_empty() {
            return None;
        }
        return Some(format!("{base}/{id}AAAAd/{name}.gif"));
    }
    if host.ends_with("giphy.com") {
        let (dir, file) = path.rsplit_once('/')?;
        if file.ends_with(".mp4") {
            return Some(format!("{dir}/giphy.gif"));
        }
    }
    None
}

/// Cosa mostrare per l'anteprima di un link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedPreview {
    /// Indirizzi da provare in ordine (la GIF animata, poi l'immagine statica).
    pub sources: Vec<String>,
    pub width: u32,
    pub height: u32,
    /// Nome del servizio da mostrare sotto (es. "Tenor").
    pub provider: Option<String>,
    /// Indirizzo della pagina: se il messaggio contiene solo questo, il testo
    /// del link si nasconde (come fa Discord).
    pub link: Option<String>,
}

/// Anteprima visibile di un embed: GIF (`gifv`) e immagini (`image`).
/// Gli altri tipi (articoli, video, …) per ora non si mostrano.
pub fn embed_preview(e: &crate::discord::models::Embed) -> Option<EmbedPreview> {
    use crate::discord::models::EmbedMedia;
    let proxied = |m: &EmbedMedia| -> Option<String> {
        // Meglio la copia sul proxy di Discord: non espone l'IP al sito esterno.
        m.proxy_url
            .clone()
            .filter(|u| check_url(u).is_ok())
            .or_else(|| Some(m.url.clone()).filter(|u| check_url(u).is_ok()))
    };
    let dims = |m: &EmbedMedia| match (m.width, m.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
        _ => None,
    };
    let provider = e.provider.as_ref().and_then(|p| p.name.clone());
    match e.kind.as_deref() {
        Some("gifv") => {
            let video = e.video.as_ref();
            let thumb = e.thumbnail.as_ref();
            let (width, height) = video.and_then(dims).or_else(|| thumb.and_then(dims))?;
            let mut sources = Vec::new();
            if let Some(thumb) = thumb.filter(|t| t.url == crate::demo::DEMO_GIF_URL) {
                sources.push(thumb.url.clone());
            }
            // Se la miniatura è già animata (GIF o WebP, es. Klipy) si prende
            // dal proxy di Discord: niente richieste al sito esterno.
            if let Some(animated) = thumb.filter(|t| maybe_animated(&t.url)).and_then(proxied) {
                sources.push(animated);
            }
            if let Some(gif) = video.and_then(|v| gif_from_video(&v.url)) {
                sources.push(gif);
            }
            if let Some(still) = thumb.and_then(proxied).filter(|s| !sources.contains(s)) {
                sources.push(still);
            }
            (!sources.is_empty()).then(|| EmbedPreview {
                sources,
                width,
                height,
                provider,
                link: e.url.clone(),
            })
        }
        Some("image") => {
            let media = e.image.as_ref().or(e.thumbnail.as_ref())?;
            let (width, height) = dims(media)?;
            let source = proxied(media)?;
            Some(EmbedPreview {
                sources: vec![source],
                width,
                height,
                provider,
                link: e.url.clone(),
            })
        }
        _ => None,
    }
}

/// Il formato dall'indirizzo (senza parametri) può essere animato.
fn maybe_animated(url: &str) -> bool {
    let path = url[..url.find(['?', '#']).unwrap_or(url.len())].to_ascii_lowercase();
    path.ends_with(".gif") || path.ends_with(".webp")
}

/// I byte sono di un formato che può essere animato (GIF o WebP).
pub fn is_animatable(bytes: &[u8]) -> bool {
    bytes.starts_with(b"GIF8") || (bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
}

/// Un fotogramma decodificato: RGBA, dimensioni e durata.
#[derive(Debug, Clone)]
pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub delay: Duration,
}

/// Decodifica una GIF o una WebP animata in fotogrammi ridotti a
/// `max_w`×`max_h` (al massimo). Restituisce `None` se il formato non è
/// animabile o se c'è un solo fotogramma: in quel caso si mostra come
/// un'immagine normale.
pub fn decode_animation(bytes: &[u8], max_w: u32, max_h: u32) -> Result<Option<Vec<RawFrame>>> {
    use image::codecs::gif::GifDecoder;
    use image::codecs::webp::WebPDecoder;
    use image::{AnimationDecoder, ImageDecoder};

    let cursor = std::io::Cursor::new(bytes);
    if bytes.starts_with(b"GIF8") {
        let decoder = GifDecoder::new(cursor)?;
        let (w, h) = decoder.dimensions();
        check_side(w, h)?;
        return decode_frames(decoder.into_frames(), w, h, max_w, max_h);
    }
    if is_animatable(bytes) {
        let decoder = WebPDecoder::new(cursor)?;
        if !decoder.has_animation() {
            return Ok(None);
        }
        let (w, h) = decoder.dimensions();
        check_side(w, h)?;
        return decode_frames(decoder.into_frames(), w, h, max_w, max_h);
    }
    Ok(None)
}

fn check_side(w: u32, h: u32) -> Result<()> {
    if w == 0 || h == 0 || w > GIF_MAX_SIDE || h > GIF_MAX_SIDE {
        bail!("animazione di dimensioni non gestite ({w}×{h})");
    }
    Ok(())
}

fn decode_frames(
    source: image::Frames<'_>,
    w: u32,
    h: u32,
    max_w: u32,
    max_h: u32,
) -> Result<Option<Vec<RawFrame>>> {
    use image::imageops;
    let scale = (max_w.max(1) as f32 / w as f32)
        .min(max_h.max(1) as f32 / h as f32)
        .min(1.0);
    let (tw, th) = (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    );

    let mut frames = Vec::new();
    let mut used = 0usize;
    for frame in source {
        let frame = frame?;
        let (num, den) = frame.delay().numer_denom_ms();
        let mut ms = num.checked_div(den).unwrap_or(100);
        // Come i browser: ritardi quasi nulli diventano 100 ms.
        if ms < 20 {
            ms = 100;
        }
        let buffer = frame.into_buffer();
        let buffer = if scale < 1.0 {
            imageops::resize(&buffer, tw, th, imageops::FilterType::Triangle)
        } else {
            buffer
        };
        let (fw, fh) = buffer.dimensions();
        let rgba = buffer.into_raw();
        used += rgba.len();
        if used > GIF_MAX_DECODED_BYTES && !frames.is_empty() {
            break;
        }
        frames.push(RawFrame {
            width: fw,
            height: fh,
            rgba,
            delay: Duration::from_millis(ms as u64),
        });
        if frames.len() >= GIF_MAX_FRAMES {
            break;
        }
    }
    Ok((frames.len() > 1).then_some(frames))
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
    fn gif_ricavata_dal_video() {
        assert_eq!(
            gif_from_video("https://media.tenor.com/0AVbKGY_MxMAAAPo/check-mate.mp4").as_deref(),
            Some("https://media.tenor.com/0AVbKGY_MxMAAAAd/check-mate.gif")
        );
        assert_eq!(
            gif_from_video("https://media0.giphy.com/media/v1.Y2lk/abc123/giphy.mp4?cid=x&rid=giphy.mp4")
                .as_deref(),
            Some("https://media0.giphy.com/media/v1.Y2lk/abc123/giphy.gif")
        );
        assert!(gif_from_video("https://media.tenor.com/abcAAAAC/x.gif").is_none());
        assert!(gif_from_video("https://evil.example/abcAAAPo/x.mp4").is_none());
        assert!(gif_from_video("http://media.tenor.com/abcAAAPo/x.mp4").is_none());
    }

    /// Prova a mano su file veri: `FERROCORD_ANIM=percorso cargo test -- --ignored animazione_da_file`.
    #[test]
    #[ignore]
    fn animazione_da_file() {
        let Ok(paths) = std::env::var("FERROCORD_ANIM") else {
            return;
        };
        for path in paths.split(';') {
            let bytes = std::fs::read(path).unwrap();
            let frames = decode_animation(&bytes, 220, 220).unwrap().expect("non animata");
            let first = &frames[0];
            println!("{path}: {} fotogrammi, {}×{}", frames.len(), first.width, first.height);
        }
    }

    #[test]
    fn gif_di_klipy_dal_proxy_di_discord() {
        let e: crate::discord::models::Embed = serde_json::from_value(serde_json::json!({
            "type": "gifv", "url": "https://klipy.com/gifs/gatto", "provider": {"name": "Klipy"},
            "thumbnail": {"url": "https://static.klipy.com/ii/abc/x.gif",
                "proxy_url": "https://images-ext-1.discordapp.net/external/k/https/static.klipy.com/ii/abc/x.gif",
                "width": 220, "height": 124},
            "video": {"url": "https://static.klipy.com/ii/abc/y.mp4", "width": 220, "height": 124}
        }))
        .unwrap();
        let p = embed_preview(&e).unwrap();
        // Miniatura già animata: dal proxy, una volta sola.
        assert_eq!(
            p.sources,
            vec!["https://images-ext-1.discordapp.net/external/k/https/static.klipy.com/ii/abc/x.gif"]
        );
        assert!(is_animatable(b"GIF89a......"));
        assert!(is_animatable(b"RIFF\0\0\0\0WEBPVP8X"));
        assert!(!is_animatable(b"\x89PNG\r\n\x1a\n...."));
    }

    #[test]
    fn anteprime_dei_link() {
        use crate::discord::models::Embed;
        let gifv: Embed = serde_json::from_str(
            r#"{"type":"gifv","url":"https://tenor.com/view/x-1","provider":{"name":"Tenor"},
                "thumbnail":{"url":"https://media.tenor.com/abcAAAAe/x.png","proxy_url":"https://images-ext-1.discordapp.net/external/s/https/media.tenor.com/abcAAAAe/x.png","width":498,"height":280},
                "video":{"url":"https://media.tenor.com/abcAAAPo/x.mp4","width":498,"height":280}}"#,
        )
        .unwrap();
        let p = embed_preview(&gifv).unwrap();
        assert_eq!(
            p.sources,
            [
                "https://media.tenor.com/abcAAAAd/x.gif",
                "https://images-ext-1.discordapp.net/external/s/https/media.tenor.com/abcAAAAe/x.png"
            ]
        );
        assert_eq!((p.width, p.height), (498, 280));
        assert_eq!(p.provider.as_deref(), Some("Tenor"));

        // Immagine da un sito qualsiasi: solo tramite il proxy di Discord.
        let image: Embed = serde_json::from_str(
            r#"{"type":"image","url":"https://example.com/a.gif",
                "thumbnail":{"url":"https://example.com/a.gif","proxy_url":"https://images-ext-2.discordapp.net/external/s/https/example.com/a.gif","width":200,"height":100}}"#,
        )
        .unwrap();
        assert_eq!(
            embed_preview(&image).unwrap().sources,
            ["https://images-ext-2.discordapp.net/external/s/https/example.com/a.gif"]
        );
        let unproxied: Embed = serde_json::from_str(
            r#"{"type":"image","thumbnail":{"url":"https://example.com/a.gif","width":200,"height":100}}"#,
        )
        .unwrap();
        assert!(embed_preview(&unproxied).is_none());
        let article: Embed = serde_json::from_str(r#"{"type":"article","title":"x"}"#).unwrap();
        assert!(embed_preview(&article).is_none());
    }

    fn gif_di_prova(w: u32, h: u32, frames: usize, delay_ms: u32) -> Vec<u8> {
        use image::codecs::gif::{GifEncoder, Repeat};
        use image::{Delay, Frame, Rgba, RgbaImage};
        let mut out = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut out);
            enc.set_repeat(Repeat::Infinite).unwrap();
            for i in 0..frames {
                let img = RgbaImage::from_pixel(w, h, Rgba([(i * 40) as u8, 100, 200, 255]));
                enc.encode_frame(Frame::from_parts(
                    img,
                    0,
                    0,
                    Delay::from_numer_denom_ms(delay_ms, 1),
                ))
                .unwrap();
            }
        }
        out
    }

    #[test]
    fn gif_animata_decodificata_e_ridotta() {
        let bytes = gif_di_prova(800, 400, 4, 50);
        let frames = decode_animation(&bytes, 400, 300).unwrap().unwrap();
        assert_eq!(frames.len(), 4);
        assert_eq!((frames[0].width, frames[0].height), (400, 200));
        assert_eq!(frames[0].rgba.len(), 400 * 200 * 4);
        assert_eq!(frames[0].delay, Duration::from_millis(50));
        // Ritardo nullo: 100 ms come nei browser.
        let fast = decode_animation(&gif_di_prova(10, 10, 2, 0), 400, 300)
            .unwrap()
            .unwrap();
        assert_eq!(fast[0].delay, Duration::from_millis(100));
        // Un solo fotogramma o un PNG: si mostra come immagine normale.
        assert!(decode_animation(&gif_di_prova(10, 10, 1, 50), 400, 300)
            .unwrap()
            .is_none());
        assert!(decode_animation(b"\x89PNG....", 400, 300).unwrap().is_none());
        // Dati rovinati: errore, non panic.
        assert!(decode_animation(b"GIF89a\x00\x00", 400, 300).is_err());
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
        assert!(check_url("https://media.tenor.com/abcAAAAd/x.gif").is_ok());
        assert!(check_url("https://images-ext-1.discordapp.net/external/x").is_ok());
        assert!(check_url("https://tenor.com/view/x").is_err());
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
