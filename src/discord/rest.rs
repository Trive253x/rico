//! REST API client (history, sending messages, profile).

use std::fmt::{self, Write as _};
use std::{sync::Arc, time::Duration};

use anyhow::{bail, Context, Result};
use reqwest::{
    header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE},
    Method, StatusCode,
};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

use super::models::{Channel, Emoji, GifResult, GuildNotifySettings, Message, User, UserProfile};
use super::USER_AGENT;

const API: &str = "https://discord.com/api/v10";
/// Longest timeout Discord allows (28 days).
pub const MAX_TIMEOUT_SECS: u64 = 28 * 24 * 3600;

/// What a moderator can do to a member (see [`Rest::moderate`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModAction {
    /// Server mute (`true`) or unmute in voice.
    Mute(bool),
    /// Server deafen or undeafen in voice.
    Deafen(bool),
    /// Move to another voice channel.
    Move(String),
    /// Out of the voice channel.
    Disconnect,
    /// Timeout for this many seconds, or `None` to remove it.
    Timeout(Option<u64>),
    Kick,
    Ban,
}
pub const HISTORY_PAGE: usize = 50;

/// Most files in one message (Discord's limit).
pub const MAX_UPLOAD_FILES: usize = 10;
/// Most bytes of all the files of one message: Discord's limit without Nitro
/// (10 MiB). Bigger uploads are refused here, before reading them.
pub const MAX_UPLOAD_BYTES: u64 = 10 * 1024 * 1024;
/// Longest message text Discord accepts without Nitro.
pub const MAX_MESSAGE_CHARS: usize = 2000;

/// Body of a request.
enum Body<'a> {
    Json(&'a Value),
    Multipart(Multipart),
}

/// A `multipart/form-data` body, ready to send.
struct Multipart {
    content_type: String,
    bytes: bytes::Bytes,
}

/// A file to upload: the name Discord shows (no path) and its bytes.
#[derive(Clone)]
pub struct Upload {
    pub filename: String,
    pub bytes: bytes::Bytes,
}

impl fmt::Debug for Upload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never the content: only the name and the size.
        write!(f, "Upload({:?}, {} bytes)", self.filename, self.bytes.len())
    }
}

/// What to send in a channel.
#[derive(Debug, Clone, Default)]
pub struct Outgoing {
    pub content: String,
    /// Message being replied to.
    pub reply_to: Option<String>,
    /// The author of the message being replied to gets a notification.
    pub ping_reply: bool,
    pub files: Vec<Upload>,
}

/// Cleans up a pasted token (spaces, quotes) and rejects unusable ones.
pub fn clean_token(raw: &str) -> std::result::Result<String, String> {
    let token = raw.trim().trim_matches('"').trim().to_string();
    if token.is_empty() {
        return Err(crate::i18n::t("Paste the token first.", "Incolla prima il token.").into());
    }
    if token.starts_with("Bot ") {
        return Err(crate::i18n::t(
            "Bot tokens are not supported: a user account token is needed.",
            "I token dei bot non sono supportati: serve il token di un account utente.",
        )
        .into());
    }
    if token.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(crate::i18n::t(
            "The token contains spaces or invalid characters.",
            "Il token contiene spazi o caratteri non validi.",
        )
        .into());
    }
    Ok(token)
}

/// Error for an invalid token, recognisable with `err.is::<Unauthorized>()`.
#[derive(Debug)]
pub struct Unauthorized;

impl fmt::Display for Unauthorized {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(crate::i18n::t(
            "invalid or expired token",
            "token non valido o scaduto",
        ))
    }
}

impl std::error::Error for Unauthorized {}

/// HTTP 404: the resource does not exist (or not yet, as a stream preview).
#[derive(Debug)]
pub struct NotFound;

impl fmt::Display for NotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(crate::i18n::t("not found", "non trovato"))
    }
}

impl std::error::Error for NotFound {}

/// Any other refusal of the API: the status, Discord's error code and
/// whether it asks for a captcha, recognisable with `err.downcast_ref`.
#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    /// Discord's JSON error code (`code`), if any.
    pub code: Option<u64>,
    /// Discord wants a captcha solved (Rico never solves one).
    pub captcha: bool,
    /// The start of the answer, for the log and the status bar.
    text: String,
}

impl ApiError {
    fn new(status: u16, body: &str) -> Self {
        let json: Option<Value> = serde_json::from_str(body).ok();
        Self {
            status,
            code: json.as_ref().and_then(|v| v["code"].as_u64()),
            captcha: json
                .as_ref()
                .is_some_and(|v| v.get("captcha_key").is_some() || v.get("captcha_sitekey").is_some()),
            text: body.chars().take(200).collect(),
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HTTP {}: {}", self.status, self.text)
    }
}

impl std::error::Error for ApiError {}

/// Longest Discord username.
const MAX_USERNAME: usize = 32;

/// The username to send a friend request to, as Discord writes it (lower
/// case): 2–32 letters, digits, `_` and `.`. Display names are not
/// usernames: the error says so.
pub fn friend_username(raw: &str) -> std::result::Result<String, String> {
    use crate::i18n::t;
    let name = raw.trim().trim_start_matches('@').to_lowercase();
    if name.is_empty() {
        return Err(t("Write a username first.", "Scrivi prima un nome utente.").into());
    }
    if name.contains('#') {
        return Err(t(
            "Old names with # are no longer used: write the username.",
            "I vecchi nomi con # non si usano più: scrivi il nome utente.",
        )
        .into());
    }
    let ok = (2..=MAX_USERNAME).contains(&name.chars().count())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
    if !ok {
        return Err(t(
            "Not a username: 2 to 32 letters, digits, _ or . (the display name does not work).",
            "Non è un nome utente: da 2 a 32 lettere, cifre, _ o . (il nome visualizzato non va bene).",
        )
        .into());
    }
    Ok(name)
}

/// What to tell the user when a friend action fails.
pub fn friend_error(e: &anyhow::Error) -> String {
    use crate::i18n::t;
    let Some(api) = e.downcast_ref::<ApiError>() else {
        return format!("{e:#}");
    };
    if api.captcha {
        return t(
            "Discord asks for a captcha for this: do it once from the official client.",
            "Discord chiede un captcha per questa azione: falla una volta dal client ufficiale.",
        )
        .into();
    }
    match api.code {
        Some(80004) => t(
            "No one has this username: check it (it is not the display name).",
            "Nessuno ha questo nome utente: controllalo (non è il nome visualizzato).",
        )
        .into(),
        Some(80000) => t(
            "This person does not accept friend requests.",
            "Questa persona non accetta richieste di amicizia.",
        )
        .into(),
        Some(80003) => t(
            "You cannot add yourself as a friend.",
            "Non puoi aggiungere te stesso agli amici.",
        )
        .into(),
        Some(80007) => t("You are already friends.", "Siete già amici.").into(),
        _ => format!("{e:#}"),
    }
}

/// Cheap to clone: the HTTP client shares the connection pool.
#[derive(Clone)]
pub struct Rest {
    http: reqwest::Client,
    auth: Arc<HeaderValue>,
}

impl Rest {
    pub fn new(token: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(20))
            // The token goes only to the API: a redirect is an error, never
            // followed (wherever it points).
            .redirect(reqwest::redirect::Policy::none())
            .https_only(true)
            .build()?;
        let mut auth = HeaderValue::from_str(token)?;
        // Keeps the token out of any reqwest debug log.
        auth.set_sensitive(true);
        Ok(Self {
            http,
            auth: Arc::new(auth),
        })
    }

    async fn call<T: DeserializeOwned>(&self, method: Method, path: &str, body: Option<&Value>) -> Result<T> {
        self.request(method, path, body.map(Body::Json)).await
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Body<'_>>,
    ) -> Result<T> {
        // Rate limit: on 429 wait `retry_after` and try again (at most 3
        // times). A 429 means Discord did not carry out the request, so trying
        // again cannot duplicate a message.
        for _ in 0..3 {
            let mut req = self
                .http
                .request(method.clone(), format!("{API}{path}"))
                .header(AUTHORIZATION, (*self.auth).clone());
            match &body {
                Some(Body::Json(v)) => req = req.json(v),
                Some(Body::Multipart(m)) => {
                    req = req
                        .header(CONTENT_TYPE, m.content_type.as_str())
                        // `Bytes`: each attempt shares the same buffer, no copy.
                        .body(m.bytes.clone());
                }
                None => {}
            }
            let resp = req.send().await?;
            let status = resp.status();

            if status == StatusCode::TOO_MANY_REQUESTS {
                let retry = resp
                    .json::<Value>()
                    .await
                    .ok()
                    .and_then(|v| v["retry_after"].as_f64())
                    .unwrap_or(1.0);
                tokio::time::sleep(Duration::from_secs_f64(retry.clamp(0.1, 30.0))).await;
                continue;
            }
            if status == StatusCode::UNAUTHORIZED {
                return Err(Unauthorized.into());
            }
            if status == StatusCode::NOT_FOUND {
                return Err(NotFound.into());
            }
            if status == StatusCode::FORBIDDEN {
                bail!(crate::i18n::t(
                    "Discord refused: you don't have the permission",
                    "Discord ha rifiutato: non hai il permesso"
                ));
            }
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(ApiError::new(status.as_u16(), &text).into());
            }
            // 204 No Content (kick, ban…): an empty body reads as `null`.
            let body = resp.bytes().await?;
            let body: &[u8] = if body.is_empty() { b"null" } else { &body };
            return Ok(serde_json::from_slice::<T>(body)?);
        }
        bail!(crate::i18n::t(
            "too many requests: try again shortly",
            "troppe richieste: riprova tra poco"
        ))
    }

    pub async fn me(&self) -> Result<User> {
        self.call(Method::GET, "/users/@me", None).await
    }

    /// Profile of a user, with the server part if `guild_id` is given. Asked
    /// only when the user opens a profile.
    pub async fn profile(&self, user_id: &str, guild_id: Option<&str>) -> Result<UserProfile> {
        if !crate::media::valid_id(user_id) || guild_id.is_some_and(|g| !crate::media::valid_id(g)) {
            anyhow::bail!("invalid id");
        }
        let mut path = format!("/users/{user_id}/profile?with_mutual_guilds=false&with_mutual_friends=false");
        if let Some(g) = guild_id {
            path.push_str("&guild_id=");
            path.push_str(g);
        }
        self.call(Method::GET, &path, None).await
    }

    /// Address of the preview image of a Go Live stream, updated by Discord
    /// every few minutes. `None` while there is no preview yet. Asked only
    /// when the user opens it.
    pub async fn stream_preview(
        &self,
        guild_id: &str,
        channel_id: &str,
        user_id: &str,
    ) -> Result<Option<String>> {
        #[derive(Deserialize)]
        struct Preview {
            url: String,
        }
        let key = crate::media::stream_key(guild_id, channel_id, user_id).context("invalid id")?;
        match self
            .call::<Preview>(Method::GET, &format!("/streams/{key}/preview"), None)
            .await
        {
            Ok(p) if crate::media::is_stream_preview_url(&p.url) => Ok(Some(p.url)),
            Ok(_) => anyhow::bail!("unexpected preview address"),
            Err(e) if e.is::<NotFound>() => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Latest messages of the channel (newest first), or the ones before `before`.
    pub async fn messages(&self, channel_id: &str, before: Option<&str>) -> Result<Vec<Message>> {
        let mut path = format!("/channels/{channel_id}/messages?limit={HISTORY_PAGE}");
        if let Some(before) = before {
            path.push_str("&before=");
            path.push_str(before);
        }
        self.call(Method::GET, &path, None).await
    }

    /// Trending GIFs (empty `query`) or search results, in the small format
    /// that fits the picker grid.
    pub async fn gifs(&self, query: &str) -> Result<Vec<GifResult>> {
        let query = query.trim();
        let path = if query.is_empty() {
            format!(
                "/gifs/trending-gifs?media_format=tinygif&provider=tenor&locale={}",
                crate::i18n::lang().code()
            )
        } else {
            format!(
                "/gifs/search?q={}&media_format=tinygif&provider=tenor&locale={}",
                encode_query(query),
                crate::i18n::lang().code()
            )
        };
        self.call(Method::GET, &path, None).await
    }

    /// Our account settings (type 1, with the server folders), base64 protobuf.
    pub async fn settings_proto(&self) -> Result<String> {
        let v: Value = self
            .call(Method::GET, "/users/@me/settings-proto/1", None)
            .await?;
        match v["settings"].as_str() {
            Some(s) => Ok(s.to_string()),
            None => bail!("settings missing from the answer"),
        }
    }

    /// Changes account settings: `settings` holds only the top-level fields
    /// to replace. With `required_data_version` Discord refuses the change if
    /// the settings changed meanwhile (`out_of_date`). Returns the settings
    /// after the change (or the current ones) and whether it was refused.
    pub async fn update_settings_proto(
        &self,
        settings: &str,
        required_data_version: Option<u32>,
    ) -> Result<(String, bool)> {
        let mut body = json!({ "settings": settings });
        if let Some(v) = required_data_version {
            body["required_data_version"] = json!(v);
        }
        let v: Value = self
            .call(Method::PATCH, "/users/@me/settings-proto/1", Some(&body))
            .await?;
        let out_of_date = v["out_of_date"].as_bool().unwrap_or(false);
        match v["settings"].as_str() {
            Some(s) => Ok((s.to_string(), out_of_date)),
            None => bail!("settings missing from the answer"),
        }
    }

    /// A moderation action on a member of a server, asked by the user from
    /// the user card (after a confirmation for the ones that remove someone).
    pub async fn moderate(&self, guild_id: &str, user_id: &str, action: &ModAction) -> Result<()> {
        if !crate::media::valid_id(guild_id) || !crate::media::valid_id(user_id) {
            bail!("invalid id");
        }
        let member = format!("/guilds/{guild_id}/members/{user_id}");
        let (method, path, body) = match action {
            ModAction::Mute(on) => (Method::PATCH, member, Some(json!({ "mute": on }))),
            ModAction::Deafen(on) => (Method::PATCH, member, Some(json!({ "deaf": on }))),
            ModAction::Move(channel_id) => {
                if !crate::media::valid_id(channel_id) {
                    bail!("invalid id");
                }
                (Method::PATCH, member, Some(json!({ "channel_id": channel_id })))
            }
            ModAction::Disconnect => (Method::PATCH, member, Some(json!({ "channel_id": null }))),
            ModAction::Timeout(seconds) => {
                let until = seconds.map(|s| {
                    let s = s.clamp(1, MAX_TIMEOUT_SECS);
                    (chrono::Utc::now() + chrono::Duration::seconds(s as i64)).to_rfc3339()
                });
                (
                    Method::PATCH,
                    member,
                    Some(json!({ "communication_disabled_until": until })),
                )
            }
            ModAction::Kick => (Method::DELETE, member, None),
            ModAction::Ban => (
                Method::PUT,
                format!("/guilds/{guild_id}/bans/{user_id}"),
                Some(json!({ "delete_message_seconds": 0 })),
            ),
        };
        self.call::<serde::de::IgnoredAny>(method, &path, body.as_ref())
            .await
            .map(|_| ())
    }

    /// Sends a message, with an optional reply and files. Never tried again
    /// after an uncertain outcome (timeout, connection lost): it could arrive
    /// twice.
    pub async fn send_message(&self, channel_id: &str, out: &Outgoing) -> Result<Message> {
        if !crate::media::valid_id(channel_id) {
            bail!("invalid id");
        }
        let payload = message_payload(channel_id, out)?;
        let path = format!("/channels/{channel_id}/messages");
        if out.files.is_empty() {
            self.call(Method::POST, &path, Some(&payload)).await
        } else {
            let body = multipart(&payload, &out.files)?;
            self.request(Method::POST, &path, Some(Body::Multipart(body)))
                .await
        }
    }

    /// The direct messages with someone: Discord gives back the existing
    /// conversation, or makes it. Only on a click of the user.
    pub async fn open_dm(&self, user_id: &str) -> Result<Channel> {
        if !crate::media::valid_id(user_id) {
            bail!("invalid id");
        }
        let body = json!({ "recipients": [user_id] });
        self.call(Method::POST, "/users/@me/channels", Some(&body)).await
    }

    /// Sends a friend request by username (only on a click of the user).
    pub async fn send_friend_request(&self, username: &str) -> Result<()> {
        let username = friend_username(username).map_err(anyhow::Error::msg)?;
        let body = json!({ "username": username, "discriminator": null });
        self.call::<Value>(Method::POST, "/users/@me/relationships", Some(&body))
            .await
            .map(drop)
    }

    /// Accepts a friend request (or asks the person to be friends).
    pub async fn accept_friend(&self, user_id: &str) -> Result<()> {
        if !crate::media::valid_id(user_id) {
            bail!("invalid id");
        }
        let body = json!({});
        self.call::<Value>(
            Method::PUT,
            &format!("/users/@me/relationships/{user_id}"),
            Some(&body),
        )
        .await
        .map(drop)
    }

    /// Ends a relationship: removes a friend, declines or cancels a request,
    /// unblocks.
    pub async fn remove_relationship(&self, user_id: &str) -> Result<()> {
        if !crate::media::valid_id(user_id) {
            bail!("invalid id");
        }
        self.call::<Value>(
            Method::DELETE,
            &format!("/users/@me/relationships/{user_id}"),
            None,
        )
        .await
        .map(drop)
    }

    /// Marks a channel read up to `message_id`, as the official client does
    /// when a channel is looked at, so the other devices agree.
    pub async fn ack(&self, channel_id: &str, message_id: &str) -> Result<()> {
        if !crate::media::valid_id(channel_id) || !crate::media::valid_id(message_id) {
            bail!("invalid id");
        }
        let body = json!({ "token": null });
        self.call::<Value>(
            Method::POST,
            &format!("/channels/{channel_id}/messages/{message_id}/ack"),
            Some(&body),
        )
        .await
        .map(drop)
    }

    /// Marks several channels read at once ("Mark as read" on a server):
    /// (channel, newest message), at most [`MAX_BULK_ACKS`].
    pub async fn ack_bulk(&self, channels: &[(String, String)]) -> Result<()> {
        let states: Vec<Value> = channels
            .iter()
            .filter(|(c, m)| crate::media::valid_id(c) && crate::media::valid_id(m))
            .take(MAX_BULK_ACKS)
            .map(|(c, m)| json!({ "channel_id": c, "message_id": m, "read_state_type": 0 }))
            .collect();
        if states.is_empty() {
            return Ok(());
        }
        let body = json!({ "read_states": states });
        self.call::<Value>(Method::POST, "/read-states/ack-bulk", Some(&body))
            .await
            .map(drop)
    }

    /// Mutes or unmutes a server for us (no notifications, no unread marks),
    /// until turned off. Discord answers with the new settings.
    pub async fn set_guild_muted(&self, guild_id: &str, muted: bool) -> Result<GuildNotifySettings> {
        if !crate::media::valid_id(guild_id) {
            bail!("invalid id");
        }
        let body = json!({ "muted": muted, "mute_config": null });
        self.call(
            Method::PATCH,
            &format!("/users/@me/guilds/{guild_id}/settings"),
            Some(&body),
        )
        .await
    }

    /// Leaves a server (only after the user confirmed it).
    pub async fn leave_guild(&self, guild_id: &str) -> Result<()> {
        if !crate::media::valid_id(guild_id) {
            bail!("invalid id");
        }
        let body = json!({ "lurking": false });
        self.call::<Value>(
            Method::DELETE,
            &format!("/users/@me/guilds/{guild_id}"),
            Some(&body),
        )
        .await
        .map(drop)
    }

    /// Changes the text of one of our messages.
    pub async fn edit_message(&self, channel_id: &str, message_id: &str, content: &str) -> Result<Message> {
        if !crate::media::valid_id(channel_id) || !crate::media::valid_id(message_id) {
            bail!("invalid id");
        }
        check_length(content)?;
        let body = json!({ "content": content });
        self.call(
            Method::PATCH,
            &format!("/channels/{channel_id}/messages/{message_id}"),
            Some(&body),
        )
        .await
    }

    /// Deletes one of our messages.
    pub async fn delete_message(&self, channel_id: &str, message_id: &str) -> Result<()> {
        if !crate::media::valid_id(channel_id) || !crate::media::valid_id(message_id) {
            bail!("invalid id");
        }
        self.call::<serde::de::IgnoredAny>(
            Method::DELETE,
            &format!("/channels/{channel_id}/messages/{message_id}"),
            None,
        )
        .await
        .map(|_| ())
    }

    /// Adds (`add`) or removes our reaction to a message.
    pub async fn react(&self, channel_id: &str, message_id: &str, emoji: &Emoji, add: bool) -> Result<()> {
        if !crate::media::valid_id(channel_id) || !crate::media::valid_id(message_id) {
            bail!("invalid id");
        }
        let emoji = reaction_path(emoji).context("invalid emoji")?;
        let method = if add { Method::PUT } else { Method::DELETE };
        self.call::<serde::de::IgnoredAny>(
            method,
            &format!("/channels/{channel_id}/messages/{message_id}/reactions/{emoji}/@me"),
            None,
        )
        .await
        .map(|_| ())
    }
}

fn check_length(content: &str) -> Result<()> {
    if content.chars().count() > MAX_MESSAGE_CHARS {
        bail!(crate::i18n::t(
            "The message is longer than 2000 characters.",
            "Il messaggio supera i 2000 caratteri."
        ));
    }
    Ok(())
}

/// The JSON of a new message. Mentions work as in Discord's client;
/// `replied_user` decides whether the author of the replied message is
/// notified.
fn message_payload(channel_id: &str, out: &Outgoing) -> Result<Value> {
    check_length(&out.content)?;
    if out.content.trim().is_empty() && out.files.is_empty() {
        bail!("empty message");
    }
    if out.files.len() > MAX_UPLOAD_FILES {
        bail!(crate::i18n::t(
            "At most 10 files per message.",
            "Al massimo 10 file per messaggio."
        ));
    }
    let total: u64 = out.files.iter().map(|f| f.bytes.len() as u64).sum();
    if total > MAX_UPLOAD_BYTES {
        bail!(crate::i18n::t(
            "The files are bigger than 10 MB.",
            "I file superano i 10 MB."
        ));
    }
    let mut body = json!({
        "content": out.content,
        "nonce": rand::random::<u64>().to_string(),
        "tts": false,
    });
    if let Some(reply) = &out.reply_to {
        if !crate::media::valid_id(reply) {
            bail!("invalid id");
        }
        body["message_reference"] = json!({
            "message_id": reply,
            "channel_id": channel_id,
            // If the message was deleted meanwhile, sent as a normal message.
            "fail_if_not_exists": false,
        });
        body["allowed_mentions"] = json!({
            "parse": ["users", "roles", "everyone"],
            "replied_user": out.ping_reply,
        });
    }
    if !out.files.is_empty() {
        body["attachments"] = Value::Array(
            out.files
                .iter()
                .enumerate()
                .map(|(i, f)| json!({ "id": i, "filename": f.filename }))
                .collect(),
        );
    }
    Ok(body)
}

/// Builds a `multipart/form-data` body: `payload_json`, then `files[n]`.
/// File names are cleaned (no paths, quotes or control characters), so they
/// cannot break the part headers.
fn multipart(payload: &Value, files: &[Upload]) -> Result<Multipart> {
    let boundary = format!("rico-{:032x}", rand::random::<u128>());
    let total: usize = files.iter().map(|f| f.bytes.len()).sum();
    let mut out = Vec::with_capacity(total + 512 + files.len() * 256);
    let json = serde_json::to_vec(payload)?;
    out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    out.extend_from_slice(b"Content-Disposition: form-data; name=\"payload_json\"\r\n");
    out.extend_from_slice(b"Content-Type: application/json\r\n\r\n");
    out.extend_from_slice(&json);
    out.extend_from_slice(b"\r\n");
    for (i, f) in files.iter().enumerate() {
        let name = upload_name(&f.filename);
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        out.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"files[{i}]\"; filename=\"{name}\"\r\n")
                .as_bytes(),
        );
        out.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
        out.extend_from_slice(&f.bytes);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok(Multipart {
        content_type: format!("multipart/form-data; boundary={boundary}"),
        bytes: out.into(),
    })
}

/// Channels marked read in one "Mark as read" (Discord's limit).
pub const MAX_BULK_ACKS: usize = 100;

/// The name of an uploaded file as Discord will show it: the file name only
/// (never the folders it was in), without characters that could break the
/// request.
pub fn upload_name(name: &str) -> String {
    let safe = crate::media::safe_filename(name);
    let clean: String = safe
        .chars()
        .map(|c| {
            if c == '"' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    if clean.is_empty() {
        "file".to_string()
    } else {
        clean
    }
}

/// The emoji in the reactions path: the character (percent-encoded) for
/// Unicode emojis, `name:id` for custom ones.
fn reaction_path(e: &Emoji) -> Option<String> {
    match (&e.id, &e.name) {
        (Some(id), name) => {
            if !crate::media::valid_id(id) {
                return None;
            }
            let name = name.as_deref().unwrap_or("_");
            if name.is_empty()
                || name.len() > 64
                || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return None;
            }
            Some(format!("{name}:{id}"))
        }
        // Unicode emojis always have characters outside ASCII: a name like
        // `..` would change the path once the URL is normalised.
        (None, Some(name)) if name.len() <= 64 && !name.is_ascii() => Some(encode_query(name)),
        _ => None,
    }
}

/// Encodes text for a query string (RFC 3986: only unreserved characters stay as they are).
fn encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn friend_usernames() {
        assert_eq!(
            friend_username("  @Ada.Lovelace_1 ").as_deref(),
            Ok("ada.lovelace_1")
        );
        for bad in [
            "",
            "a",
            "ada#1234",
            "Ada Lovelace",
            "ädä",
            &"x".repeat(33),
            "ada/../x",
        ] {
            assert!(friend_username(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn api_errors() {
        let e = ApiError::new(
            400,
            r#"{"code": 80004, "message": "No users with DiscordTag exist"}"#,
        );
        assert_eq!((e.status, e.code, e.captcha), (400, Some(80004), false));
        let e: anyhow::Error = e.into();
        assert!(friend_error(&e).contains("username") || friend_error(&e).contains("nome utente"));
        let c = ApiError::new(
            400,
            r#"{"captcha_key": ["captcha-required"], "captcha_sitekey": "x"}"#,
        );
        assert!(c.captcha);
        let raw = ApiError::new(500, "not json");
        assert_eq!(raw.code, None);
        assert_eq!(raw.to_string(), "HTTP 500: not json");
        assert_eq!(ApiError::new(500, &"y".repeat(500)).text.len(), 200);
    }

    use super::*;

    fn upload(name: &str, bytes: &[u8]) -> Upload {
        Upload {
            filename: name.into(),
            bytes: bytes::Bytes::copy_from_slice(bytes),
        }
    }

    #[test]
    fn reply_payload() {
        let out = Outgoing {
            content: "hi".into(),
            reply_to: Some("123".into()),
            ping_reply: false,
            files: vec![],
        };
        let v = message_payload("456", &out).unwrap();
        assert_eq!(v["message_reference"]["message_id"], "123");
        assert_eq!(v["message_reference"]["channel_id"], "456");
        assert_eq!(v["allowed_mentions"]["replied_user"], false);
        // Without a reply, no reference and Discord's usual mentions.
        let plain = message_payload(
            "456",
            &Outgoing {
                content: "hi".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(plain.get("message_reference").is_none());
        assert!(plain.get("allowed_mentions").is_none());
        // A made-up id is refused before any request.
        let bad = Outgoing {
            content: "hi".into(),
            reply_to: Some("../x".into()),
            ..Default::default()
        };
        assert!(message_payload("456", &bad).is_err());
    }

    #[test]
    fn payload_limits() {
        let empty = Outgoing::default();
        assert!(message_payload("1", &empty).is_err());
        let long = Outgoing {
            content: "a".repeat(MAX_MESSAGE_CHARS + 1),
            ..Default::default()
        };
        assert!(message_payload("1", &long).is_err());
        let many = Outgoing {
            files: (0..=MAX_UPLOAD_FILES)
                .map(|i| upload(&format!("{i}.txt"), b"x"))
                .collect(),
            ..Default::default()
        };
        assert!(message_payload("1", &many).is_err());
        let big = Outgoing {
            files: vec![upload("big.bin", &vec![0u8; MAX_UPLOAD_BYTES as usize + 1])],
            ..Default::default()
        };
        assert!(message_payload("1", &big).is_err());
        // Only files, no text: fine.
        let files = Outgoing {
            files: vec![upload("a.txt", b"x")],
            ..Default::default()
        };
        assert_eq!(
            message_payload("1", &files).unwrap()["attachments"][0]["filename"],
            "a.txt"
        );
    }

    #[test]
    fn multipart_body() {
        let payload = json!({"content": "x"});
        let m = multipart(&payload, &[upload("../../secret/\"a\"\r\nX: y.txt", b"DATA")]).unwrap();
        let boundary = m
            .content_type
            .strip_prefix("multipart/form-data; boundary=")
            .unwrap();
        let body = String::from_utf8(m.bytes.to_vec()).unwrap();
        assert!(body.starts_with(&format!("--{boundary}\r\n")));
        assert!(body.ends_with(&format!("--{boundary}--\r\n")));
        assert!(body.contains("name=\"payload_json\""));
        assert!(body.contains("{\"content\":\"x\"}"));
        assert!(body.contains("\r\n\r\nDATA\r\n"));
        // The file name cannot add headers, quotes or folders.
        let disposition = body.lines().find(|l| l.contains("name=\"files[0]\"")).unwrap();
        assert!(!disposition.contains("secret"));
        assert_eq!(disposition.matches('"').count(), 4, "{disposition}");
        assert!(!body.contains("\r\nX: y"));
    }

    #[test]
    fn upload_names() {
        assert_eq!(upload_name("C:\\Users\\me\\photo.png"), "photo.png");
        assert_eq!(upload_name("/home/me/doc.pdf"), "doc.pdf");
        assert_eq!(upload_name("a\"b.txt"), "a_b.txt");
    }

    #[test]
    fn reaction_paths() {
        let unicode = Emoji {
            id: None,
            name: Some("🦀".into()),
            animated: false,
        };
        assert_eq!(reaction_path(&unicode).as_deref(), Some("%F0%9F%A6%80"));
        let custom = Emoji {
            id: Some("123456789".into()),
            name: Some("ferris".into()),
            animated: false,
        };
        assert_eq!(reaction_path(&custom).as_deref(), Some("ferris:123456789"));
        // Nothing that could change the path.
        let bad_name = Emoji {
            id: Some("123".into()),
            name: Some("../x".into()),
            animated: false,
        };
        assert!(reaction_path(&bad_name).is_none());
        let bad_id = Emoji {
            id: Some("12/3".into()),
            name: Some("x".into()),
            animated: false,
        };
        assert!(reaction_path(&bad_id).is_none());
        assert!(reaction_path(&Emoji::default()).is_none());
        let slash = Emoji {
            id: None,
            name: Some("/../".into()),
            animated: false,
        };
        assert!(reaction_path(&slash).is_none());
        let dots = Emoji {
            id: None,
            name: Some("..".into()),
            animated: false,
        };
        assert!(reaction_path(&dots).is_none());
    }

    #[test]
    fn query_encoding() {
        assert_eq!(encode_query("gatto nero"), "gatto%20nero");
        assert_eq!(encode_query("a&b=c/è"), "a%26b%3Dc%2F%C3%A8");
    }

    #[test]
    fn token_cleanup() {
        assert_eq!(clean_token("  \"abc.def\" ").unwrap(), "abc.def");
        assert!(clean_token("").is_err());
        assert!(clean_token("Bot abc").is_err());
        assert!(clean_token("ab c").is_err());
    }

    /// A made-up token must be recognised as `Unauthorized`.
    /// `cargo test -- --ignored`
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn fake_token_rejected() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let err = Rest::new("test-token").unwrap().me().await.unwrap_err();
        assert!(err.is::<Unauthorized>(), "{err:#}");
    }
}
