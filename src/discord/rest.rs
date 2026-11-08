//! REST API client (history, sending messages, profile).

use std::{fmt, sync::Arc, time::Duration};

use anyhow::{bail, Result};
use reqwest::{
    header::{HeaderValue, AUTHORIZATION},
    Method, StatusCode,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use super::models::{GifResult, Message, User, UserProfile};
use super::USER_AGENT;

const API: &str = "https://discord.com/api/v10";
pub const HISTORY_PAGE: usize = 50;

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
        // Rate limit: on 429 wait `retry_after` and try again (at most 3 times).
        for _ in 0..3 {
            let mut req = self
                .http
                .request(method.clone(), format!("{API}{path}"))
                .header(AUTHORIZATION, (*self.auth).clone());
            if let Some(body) = body {
                req = req.json(body);
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
            if status == StatusCode::FORBIDDEN {
                bail!(crate::i18n::t(
                    "you don't have access to this channel",
                    "non hai accesso a questo canale"
                ));
            }
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                let text: String = text.chars().take(200).collect();
                bail!("HTTP {}: {text}", status.as_u16());
            }
            return Ok(resp.json::<T>().await?);
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

    pub async fn send_message(&self, channel_id: &str, content: &str) -> Result<Message> {
        let body = json!({
            "content": content,
            "nonce": rand::random::<u64>().to_string(),
            "tts": false,
        });
        self.call(
            Method::POST,
            &format!("/channels/{channel_id}/messages"),
            Some(&body),
        )
        .await
    }
}

/// Encodes text for a query string (RFC 3986: only unreserved characters stay as they are).
fn encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
