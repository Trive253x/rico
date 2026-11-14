//! QR code login ("Remote Auth" v2), the same one the web client uses.
//!
//! Flow:
//! 1. we generate a throwaway RSA-2048 key pair;
//! 2. the server sends an encrypted nonce: we decrypt it and send back its
//!    SHA-256 to prove we hold the private key;
//! 3. we receive a `fingerprint`: the QR holds `https://discord.com/ra/<fingerprint>`;
//! 4. when the phone scans it the profile arrives (encrypted); when it
//!    confirms, a ticket arrives, exchanged over REST for the token (also encrypted).
//!
//! The password never goes through here (see `password_login` for that).

use std::time::Duration;

use crate::i18n::t;
use anyhow::{bail, Context, Result};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use futures_util::StreamExt;
use rand::rngs::OsRng;
use rsa::{pkcs8::EncodePublicKey, Oaep, RsaPrivateKey, RsaPublicKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::{
    sync::mpsc::UnboundedSender,
    task::JoinHandle,
    time::{interval_at, timeout, Instant, Interval},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, http::HeaderValue, Message as WsMessage},
};
use tracing::info;

use super::password_login::{self, Answer};
use super::{send_json, tick_opt, USER_AGENT};

const GATEWAY_URL: &str = "wss://remote-auth-gateway.discord.gg/?v=2";
const LOGIN_URL: &str = "https://discord.com/api/v9/users/@me/remote-auth/login";

#[derive(Debug, Clone)]
pub enum AuthEvent {
    Progress(String),
    /// URL to encode in the QR code.
    Qr(String),
    /// The phone scanned the code: waiting for confirmation.
    Scanned(String),
    Token(String),
    Failed(String),
}

/// Starts the QR login on a tokio task; events arrive on `tx` converted to `E`.
pub fn spawn<E: From<AuthEvent> + Send + 'static>(tx: UnboundedSender<E>) -> JoinHandle<()> {
    tokio::spawn(run(tx))
}

/// Runs the whole QR login flow; an error becomes `AuthEvent::Failed`.
pub async fn run<E: From<AuthEvent> + Send + 'static>(tx: UnboundedSender<E>) {
    if let Err(e) = flow(&tx).await {
        info!("QR login ended: {e:#}");
        emit(&tx, AuthEvent::Failed(format!("{e:#}")));
    }
}

fn emit<E: From<AuthEvent>>(tx: &UnboundedSender<E>, ev: AuthEvent) {
    let _ = tx.send(ev.into());
}

fn decrypt(key: &RsaPrivateKey, b64: &str) -> Result<Vec<u8>> {
    let data = STANDARD.decode(b64).context("invalid base64")?;
    key.decrypt(Oaep::new::<Sha256>(), &data)
        .context("RSA decryption failed")
}

fn field<'a>(v: &'a Value, name: &str) -> Result<&'a str> {
    v[name]
        .as_str()
        .with_context(|| format!("missing field `{name}`"))
}

async fn flow<E: From<AuthEvent>>(tx: &UnboundedSender<E>) -> Result<()> {
    emit(
        tx,
        AuthEvent::Progress(
            t(
                "Generating the encryption keys…",
                "Generazione delle chiavi di cifratura…",
            )
            .into(),
        ),
    );
    let key = tokio::task::spawn_blocking(|| RsaPrivateKey::new(&mut OsRng, 2048)).await??;
    let public_key = RsaPublicKey::from(&key).to_public_key_der()?;
    let public_b64 = STANDARD.encode(public_key.as_bytes());

    emit(
        tx,
        AuthEvent::Progress(t("Connecting to Discord…", "Connessione a Discord…").into()),
    );
    let mut request = GATEWAY_URL.into_client_request()?;
    request
        .headers_mut()
        .insert("Origin", HeaderValue::from_static("https://discord.com"));
    let (ws, _) = timeout(Duration::from_secs(20), connect_async(request))
        .await
        .context("connection timed out")??;
    let (mut sink, mut stream) = ws.split();
    let mut heartbeat: Option<Interval> = None;

    loop {
        tokio::select! {
            _ = tick_opt(&mut heartbeat) => {
                send_json(&mut sink, &json!({ "op": "heartbeat" })).await?;
            }
            msg = stream.next() => {
                let text = match msg {
                    None => bail!(t("The QR code has expired", "Il codice QR è scaduto")),
                    Some(Err(e)) => return Err(e.into()),
                    Some(Ok(WsMessage::Text(t))) => t,
                    Some(Ok(WsMessage::Close(_))) => bail!(t("The QR code has expired", "Il codice QR è scaduto")),
                    Some(Ok(_)) => continue,
                };
                let v: Value = serde_json::from_str(&text)?;
                match v["op"].as_str().unwrap_or_default() {
                    "hello" => {
                        let ms = v["heartbeat_interval"].as_u64().unwrap_or(41_250).max(1000);
                        let period = Duration::from_millis(ms);
                        heartbeat = Some(interval_at(Instant::now() + period, period));
                        send_json(&mut sink, &json!({ "op": "init", "encoded_public_key": public_b64 })).await?;
                    }
                    "nonce_proof" => {
                        let nonce = decrypt(&key, field(&v, "encrypted_nonce")?)?;
                        let proof = URL_SAFE_NO_PAD.encode(Sha256::digest(&nonce));
                        send_json(&mut sink, &json!({ "op": "nonce_proof", "proof": proof })).await?;
                    }
                    "pending_remote_init" => {
                        let fingerprint = field(&v, "fingerprint")?;
                        emit(tx, AuthEvent::Qr(format!("https://discord.com/ra/{fingerprint}")));
                    }
                    "pending_ticket" => {
                        // Format: "id:discriminator:avatar:username"
                        let payload = decrypt(&key, field(&v, "encrypted_user_payload")?)?;
                        let payload = String::from_utf8_lossy(&payload);
                        let username = payload.splitn(4, ':').nth(3).unwrap_or("?").to_string();
                        emit(tx, AuthEvent::Scanned(username));
                    }
                    "pending_login" => {
                        emit(tx, AuthEvent::Progress(t("Signing in…", "Accesso in corso…").into()));
                        let token = exchange_ticket(field(&v, "ticket")?, &key).await?;
                        emit(tx, AuthEvent::Token(token));
                        return Ok(());
                    }
                    "cancel" => bail!(t("Login cancelled from the phone", "Accesso annullato dal telefono")),
                    _ => {}
                }
            }
        }
    }
}

async fn exchange_ticket(ticket: &str, key: &RsaPrivateKey) -> Result<String> {
    let http = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(20))
        // The answer holds the (encrypted) token: only from Discord's API.
        .redirect(reqwest::redirect::Policy::none())
        .https_only(true)
        .build()?;
    let resp = http
        .post(LOGIN_URL)
        .json(&json!({ "ticket": ticket }))
        .send()
        .await?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);

    // A new location or a captcha are answered as for the password login.
    if body.get("encrypted_token").is_none() {
        match password_login::classify(status.as_u16(), &body) {
            Answer::NewLocation => {
                info!("QR login: new location to authorize by email");
                bail!(password_login::new_location_notice());
            }
            Answer::Captcha => {
                info!("QR login: captcha asked");
                bail!(password_login::captcha_notice());
            }
            _ if !status.is_success() => {
                bail!("ticket exchange failed (HTTP {})", status.as_u16())
            }
            _ => {}
        }
    }
    let token = decrypt(key, field(&body, "encrypted_token")?)?;
    Ok(String::from_utf8(token)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live handshake up to the QR code: no account involved.
    /// `cargo test -- --ignored`
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn handshake_up_to_the_qr_code() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AuthEvent>();
        let handle = spawn(tx);
        let result = timeout(Duration::from_secs(30), async {
            while let Some(ev) = rx.recv().await {
                match ev {
                    AuthEvent::Qr(url) => return Ok(url),
                    AuthEvent::Failed(e) => return Err(e),
                    _ => {}
                }
            }
            Err("channel closed".to_string())
        })
        .await;
        handle.abort();
        let url = result.expect("timeout").expect("handshake failed");
        assert!(url.starts_with("https://discord.com/ra/"), "{url}");
    }
}
