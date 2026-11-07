//! Downloads of attached image previews and of GIFs from links.
//!
//! Safety rules: downloads only over HTTPS and only from Discord's servers
//! (CDN and media proxy) plus a short list of GIF hosts, with a limit on the
//! response size; the media proxy is asked for an already resized thumbnail,
//! so huge images are neither downloaded nor decoded.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::discord::models::Attachment;

/// Largest side of the preview shown in the chat (logical pixels).
pub const PREVIEW_MAX_W: u32 = 400;
pub const PREVIEW_MAX_H: u32 = 300;
/// Size limit for a single downloaded preview.
pub const PREVIEW_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Limit for the full-screen image (version resized by the proxy).
pub const VIEWER_MAX_BYTES: usize = 25 * 1024 * 1024;
/// Largest side of the image requested for the full-screen view.
const VIEWER_MAX_SIDE: u32 = 2560;
/// Limit for saving the original (Discord attachments go up to 500 MB).
const FILE_MAX_BYTES: u64 = 600 * 1024 * 1024;
/// Servers images are downloaded from: Discord's (CDN, media proxy and
/// external image proxy) and the Tenor and Giphy GIF servers, which Discord
/// only sends as MP4 videos (see `gif_from_video`).
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
    // GIFs from Discord's search (provided by Klipy).
    "static.klipy.com",
];
/// Most frames of an animated GIF and most memory they can take once
/// decoded: beyond that, the animation stops at the frames already read.
const GIF_MAX_FRAMES: usize = 300;
const GIF_MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;
/// GIFs larger than this (pixels per side) are not even decoded.
const GIF_MAX_SIDE: u32 = 4096;

/// Size at which to show the image: keeps the proportions and does not exceed
/// the preview box (nor the original).
pub fn display_size(width: u32, height: u32) -> (f32, f32) {
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    let scale = (PREVIEW_MAX_W as f32 / w).min(PREVIEW_MAX_H as f32 / h).min(1.0);
    ((w * scale).round().max(1.0), (h * scale).round().max(1.0))
}

/// Address to download: the media proxy thumbnail when there is one, otherwise the original.
/// Twice the shown size is requested, for high density screens.
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

/// Address for the full-screen view: the media proxy resizes images larger
/// than `VIEWER_MAX_SIDE`, the others come whole.
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

/// Checks that the address is HTTPS and points to an allowed server.
pub fn check_url(url: &str) -> Result<()> {
    let Some(rest) = url.strip_prefix("https://") else {
        bail!("https addresses only");
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        bail!("invalid address");
    }
    let host = authority
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !ALLOWED_HOSTS.contains(&host.as_str()) {
        bail!("server not allowed: {host}");
    }
    Ok(())
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(crate::discord::USER_AGENT)
            .timeout(Duration::from_secs(30))
            // Redirects could lead outside the allowed servers.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("HTTP client")
    })
}

/// Client to save large files: no overall timeout, but a limit on the wait
/// between one chunk and the next.
fn file_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(crate::discord::USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("HTTP client")
    })
}

/// Downloads the bytes of an image (at most `max_bytes`) following the rules above.
pub async fn fetch(url: &str, max_bytes: usize) -> Result<Vec<u8>> {
    check_url(url)?;
    let mut resp = client().get(url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        bail!("HTTP {}", status.as_u16());
    }
    if let Some(len) = resp.content_length() {
        if len as usize > max_bytes {
            bail!("image too large ({len} bytes)");
        }
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if !content_type.is_empty() && !content_type.starts_with("image/") {
        bail!("the server did not return an image ({content_type})");
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.context("download interrupted")? {
        if body.len() + chunk.len() > max_bytes {
            bail!("image too large");
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

/// GIF address from the MP4 video Discord sends for Tenor and Giphy links.
/// Tenor uses the same ID for every format and changes only the final code
/// (`AAAPo` = MP4, `AAAAd` = medium GIF); Giphy changes the file name.
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

/// What to show as the preview of a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedPreview {
    /// Addresses to try in order (the animated GIF, then the static image).
    pub sources: Vec<String>,
    pub width: u32,
    pub height: u32,
    /// Name of the service to show below (e.g. "Tenor").
    pub provider: Option<String>,
    /// Address of the page: if the message contains only this, the link text
    /// is hidden (as Discord does).
    pub link: Option<String>,
}

/// Visible preview of an embed: GIFs (`gifv`) and images (`image`).
/// Other types (articles, videos, …) are not shown for now.
pub fn embed_preview(e: &crate::discord::models::Embed) -> Option<EmbedPreview> {
    use crate::discord::models::EmbedMedia;
    let proxied = |m: &EmbedMedia| -> Option<String> {
        // Better the copy on Discord's proxy: it does not expose the IP to the external site.
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
            // If the thumbnail is already animated (GIF or WebP, e.g. Klipy) it
            // is taken from Discord's proxy: no requests to the external site.
            // Without parameters the proxy returns only the first frame:
            // `animated=true` is needed for the whole animation.
            if let Some(t) = thumb.filter(|t| maybe_animated(&t.url)) {
                if let Some(url) = proxied(t) {
                    let sep = if url.contains('?') { '&' } else { '?' };
                    let size = dims(t)
                        .map(|(w, h)| format!("&width={w}&height={h}"))
                        .unwrap_or_default();
                    sources.push(format!("{url}{sep}animated=true{size}"));
                }
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

/// Image of a custom emoji on Discord's CDN, at 48 px.
pub fn emoji_url(id: &str, animated: bool) -> String {
    let ext = if animated { "gif" } else { "png" };
    format!("https://cdn.discordapp.com/emojis/{id}.{ext}?size=48")
}

/// The format in the address (without parameters) can be animated.
fn maybe_animated(url: &str) -> bool {
    let path = url[..url.find(['?', '#']).unwrap_or(url.len())].to_ascii_lowercase();
    path.ends_with(".gif") || path.ends_with(".webp")
}

/// The bytes are in a format that can be animated (GIF or WebP).
pub fn is_animatable(bytes: &[u8]) -> bool {
    bytes.starts_with(b"GIF8") || (bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
}

/// A decoded frame: RGBA, size and duration.
#[derive(Debug, Clone)]
pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub delay: Duration,
}

/// Decodes an animated GIF or WebP into frames scaled down to at most
/// `max_w`×`max_h`. Returns `None` if the format cannot be animated or
/// there is a single frame: then it is shown as a normal image.
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
        bail!("animation size not supported ({w}×{h})");
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
        // Like browsers: near-zero delays become 100 ms.
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

/// Safe file name from the one chosen by whoever uploaded the attachment:
/// no paths, characters forbidden on Windows, reserved names or trailing
/// spaces and dots; limited length, extension kept.
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
        out = "image".to_string();
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

/// First free path in the folder: `photo.jpg`, `photo (1).jpg`, `photo (2).jpg`…
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

/// Folder where downloaded files are saved: the user's Downloads, if it exists.
pub fn download_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|d| {
            d.download_dir()
                .map(Path::to_path_buf)
                .or_else(|| Some(d.home_dir().to_path_buf()))
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Saves the original attachment in `dir`. Writes a `.part` file first and
/// renames it only when the download is complete: a half file never keeps the right name.
pub async fn save_to(url: &str, dir: &Path, filename: &str) -> Result<PathBuf> {
    use tokio::io::AsyncWriteExt;

    check_url(url)?;
    let mut resp = file_client().get(url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        bail!("HTTP {}", status.as_u16());
    }
    if resp.content_length().is_some_and(|len| len > FILE_MAX_BYTES) {
        bail!("file too large");
    }
    tokio::fs::create_dir_all(dir).await?;
    let name = safe_filename(filename);
    let part = unique_path(dir, &format!("{name}.part"));
    let result = async {
        let mut file = tokio::fs::File::create(&part).await?;
        let mut written: u64 = 0;
        while let Some(chunk) = resp.chunk().await.context("download interrupted")? {
            written += chunk.len() as u64;
            if written > FILE_MAX_BYTES {
                bail!("file too large");
            }
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        drop(file);
        // The final name is chosen when the download ends: meanwhile a file
        // with the same name could have appeared.
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

/// Fake image for the demo mode (no connection): a gradient with a grid,
/// in RGBA.
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
    fn proportional_size() {
        assert_eq!(display_size(800, 600), (400.0, 300.0));
        assert_eq!(display_size(1000, 200), (400.0, 80.0));
        assert_eq!(display_size(100, 50), (100.0, 50.0));
        assert_eq!(display_size(0, 0), (1.0, 1.0));
    }

    #[test]
    fn thumbnail_from_the_media_proxy() {
        let a = att(
            r#"{"url":"https://cdn.discordapp.com/attachments/1/2/a.png?ex=1&hm=2","proxy_url":"https://media.discordapp.net/attachments/1/2/a.png?ex=1&hm=2","width":4000,"height":3000}"#,
        );
        assert_eq!(
            preview_url(&a).unwrap(),
            "https://media.discordapp.net/attachments/1/2/a.png?ex=1&hm=2&width=800&height=600"
        );
        // Image already small: no resizing.
        let small = att(r#"{"proxy_url":"https://media.discordapp.net/a.png","width":100,"height":80}"#);
        assert_eq!(preview_url(&small).unwrap(), "https://media.discordapp.net/a.png");
        // Without dimensions it is not an image to show.
        assert!(preview_url(&att(r#"{"url":"https://cdn.discordapp.com/x.png"}"#)).is_none());
    }

    #[test]
    fn gif_derived_from_the_video() {
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

    /// Manual test on real files: `FERROCORD_ANIM=path cargo test -- --ignored animation_from_file`.
    #[test]
    #[ignore]
    fn animation_from_file() {
        let Ok(paths) = std::env::var("FERROCORD_ANIM") else {
            return;
        };
        for path in paths.split(';') {
            let bytes = std::fs::read(path).unwrap();
            let frames = decode_animation(&bytes, 220, 220).unwrap().expect("not animated");
            let first = &frames[0];
            println!(
                "{path}: {} frames, {}×{}",
                frames.len(),
                first.width,
                first.height
            );
        }
    }

    #[test]
    fn klipy_gif_from_discord_proxy() {
        let e: crate::discord::models::Embed = serde_json::from_value(serde_json::json!({
            "type": "gifv", "url": "https://klipy.com/gifs/gatto", "provider": {"name": "Klipy"},
            "thumbnail": {"url": "https://static.klipy.com/ii/abc/x.gif",
                "proxy_url": "https://images-ext-1.discordapp.net/external/k/https/static.klipy.com/ii/abc/x.gif",
                "width": 220, "height": 124},
            "video": {"url": "https://static.klipy.com/ii/abc/y.mp4", "width": 220, "height": 124}
        }))
        .unwrap();
        let p = embed_preview(&e).unwrap();
        // Thumbnail already animated: from the proxy, asking for the whole
        // animation; then the same as a static image, if the animation fails.
        assert_eq!(
            p.sources,
            vec![
                "https://images-ext-1.discordapp.net/external/k/https/static.klipy.com/ii/abc/x.gif?animated=true&width=220&height=124",
                "https://images-ext-1.discordapp.net/external/k/https/static.klipy.com/ii/abc/x.gif",
            ]
        );
        assert!(is_animatable(b"GIF89a......"));
        assert!(is_animatable(b"RIFF\0\0\0\0WEBPVP8X"));
        assert!(!is_animatable(b"\x89PNG\r\n\x1a\n...."));
    }

    #[test]
    fn link_previews() {
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

        // Image from any site: only through Discord's proxy.
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
    fn animated_gif_decoded_and_scaled_down() {
        let bytes = gif_di_prova(800, 400, 4, 50);
        let frames = decode_animation(&bytes, 400, 300).unwrap().unwrap();
        assert_eq!(frames.len(), 4);
        assert_eq!((frames[0].width, frames[0].height), (400, 200));
        assert_eq!(frames[0].rgba.len(), 400 * 200 * 4);
        assert_eq!(frames[0].delay, Duration::from_millis(50));
        // Zero delay: 100 ms as in browsers.
        let fast = decode_animation(&gif_di_prova(10, 10, 2, 0), 400, 300)
            .unwrap()
            .unwrap();
        assert_eq!(fast[0].delay, Duration::from_millis(100));
        // A single frame or a PNG: shown as a normal image.
        assert!(decode_animation(&gif_di_prova(10, 10, 1, 50), 400, 300)
            .unwrap()
            .is_none());
        assert!(decode_animation(b"\x89PNG....", 400, 300).unwrap().is_none());
        // Broken data: an error, not a panic.
        assert!(decode_animation(b"GIF89a\x00\x00", 400, 300).is_err());
    }

    #[test]
    fn large_image_for_the_full_view() {
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
    fn safe_file_names() {
        assert_eq!(safe_filename("foto.jpg"), "foto.jpg");
        assert_eq!(safe_filename("../../Windows/evil.exe"), "evil.exe");
        assert_eq!(safe_filename("..\\..\\x.png"), "x.png");
        assert_eq!(safe_filename("a<b>c:d?.png"), "a_b_c_d_.png");
        assert_eq!(safe_filename("CON.txt"), "_CON.txt");
        assert_eq!(safe_filename(".bashrc"), "bashrc");
        assert_eq!(safe_filename("nome. . "), "nome");
        assert_eq!(safe_filename(""), "image");
        assert_eq!(safe_filename("..."), "image");
        let long = format!("{}.png", "x".repeat(500));
        let safe = safe_filename(&long);
        assert!(safe.ends_with(".png") && safe.chars().count() == 120);
    }

    #[test]
    fn free_path() {
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
    fn only_allowed_servers() {
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

    /// Downloads a public Discord default avatar. `cargo test -- --ignored`
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn downloads_an_image_from_the_cdn() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let bytes = fetch(
            "https://cdn.discordapp.com/embed/avatars/0.png",
            PREVIEW_MAX_BYTES,
        )
        .await
        .unwrap();
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']), "not a PNG");
    }

    /// Saves a public default avatar in a temporary folder.
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn saves_a_file_from_the_cdn() {
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
