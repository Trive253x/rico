//! Client per le API REST (cronologia, invio messaggi, profilo).

use std::{fmt, sync::Arc, time::Duration};

use anyhow::{bail, Result};
use reqwest::{
    header::{HeaderValue, AUTHORIZATION},
    Method, StatusCode,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use super::models::{GifResult, Message, User};
use super::USER_AGENT;

const API: &str = "https://discord.com/api/v10";
pub const HISTORY_PAGE: usize = 50;

/// Pulisce un token incollato (spazi, virgolette) e scarta quelli inutilizzabili.
pub fn clean_token(raw: &str) -> std::result::Result<String, String> {
    let token = raw.trim().trim_matches('"').trim().to_string();
    if token.is_empty() {
        return Err("Incolla prima il token.".into());
    }
    if token.starts_with("Bot ") {
        return Err("I token dei bot non sono supportati: serve il token di un account utente.".into());
    }
    if token.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("Il token contiene spazi o caratteri non validi.".into());
    }
    Ok(token)
}

/// Errore specifico per token non valido, riconoscibile con `err.is::<Unauthorized>()`.
#[derive(Debug)]
pub struct Unauthorized;

impl fmt::Display for Unauthorized {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("token non valido o scaduto")
    }
}

impl std::error::Error for Unauthorized {}

/// Economico da clonare: il client HTTP condivide il pool di connessioni.
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
        // Evita che il token finisca in eventuali log di debug di reqwest.
        auth.set_sensitive(true);
        Ok(Self {
            http,
            auth: Arc::new(auth),
        })
    }

    async fn call<T: DeserializeOwned>(&self, method: Method, path: &str, body: Option<&Value>) -> Result<T> {
        // Rate limit: su 429 si aspetta `retry_after` e si riprova (max 3 volte).
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
                bail!("non hai accesso a questo canale");
            }
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                let text: String = text.chars().take(200).collect();
                bail!("HTTP {}: {text}", status.as_u16());
            }
            return Ok(resp.json::<T>().await?);
        }
        bail!("troppe richieste: riprova tra poco")
    }

    pub async fn me(&self) -> Result<User> {
        self.call(Method::GET, "/users/@me", None).await
    }

    /// Ultimi messaggi del canale (dal più recente), oppure quelli prima di `before`.
    pub async fn messages(&self, channel_id: &str, before: Option<&str>) -> Result<Vec<Message>> {
        let mut path = format!("/channels/{channel_id}/messages?limit={HISTORY_PAGE}");
        if let Some(before) = before {
            path.push_str("&before=");
            path.push_str(before);
        }
        self.call(Method::GET, &path, None).await
    }

    /// GIF di tendenza (`query` vuota) o risultati di una ricerca, nel formato
    /// piccolo adatto alla griglia del selettore.
    pub async fn gifs(&self, query: &str) -> Result<Vec<GifResult>> {
        let query = query.trim();
        let path = if query.is_empty() {
            "/gifs/trending-gifs?media_format=tinygif&provider=tenor&locale=it".to_string()
        } else {
            format!(
                "/gifs/search?q={}&media_format=tinygif&provider=tenor&locale=it",
                encode_query(query)
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

/// Codifica un testo per la query string (RFC 3986: solo i caratteri non riservati restano in chiaro).
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
    fn query_codificata() {
        assert_eq!(encode_query("gatto nero"), "gatto%20nero");
        assert_eq!(encode_query("a&b=c/è"), "a%26b%3Dc%2F%C3%A8");
    }

    #[test]
    fn pulizia_token() {
        assert_eq!(clean_token("  \"abc.def\" ").unwrap(), "abc.def");
        assert!(clean_token("").is_err());
        assert!(clean_token("Bot abc").is_err());
        assert!(clean_token("ab c").is_err());
    }

    /// Un token inventato deve essere riconosciuto come `Unauthorized`.
    /// `cargo test -- --ignored`
    #[tokio::test]
    #[ignore = "richiede la rete"]
    async fn token_falso_rifiutato() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let err = Rest::new("token-di-prova").unwrap().me().await.unwrap_err();
        assert!(err.is::<Unauthorized>(), "{err:#}");
    }
}
