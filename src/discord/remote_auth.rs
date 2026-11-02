//! Login con QR code ("Remote Auth" v2), lo stesso usato dal client web.
//!
//! Flusso:
//! 1. generiamo una coppia di chiavi RSA-2048 usa-e-getta;
//! 2. il server ci manda un nonce cifrato: lo decifriamo e ne rimandiamo lo
//!    SHA-256 per dimostrare di possedere la chiave privata;
//! 3. riceviamo un `fingerprint`: il QR contiene `https://discord.com/ra/<fingerprint>`;
//! 4. quando il telefono scansiona arriva il profilo (cifrato), quando conferma
//!    arriva un ticket che scambiamo via REST con il token (anch'esso cifrato).
//!
//! La password non passa mai da qui.

use std::time::Duration;

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

use super::{send_json, tick_opt, USER_AGENT};
use crate::app::AppEvent;

const GATEWAY_URL: &str = "wss://remote-auth-gateway.discord.gg/?v=2";
const LOGIN_URL: &str = "https://discord.com/api/v9/users/@me/remote-auth/login";

#[derive(Debug)]
pub enum AuthEvent {
    Progress(String),
    /// URL da codificare nel QR.
    Qr(String),
    /// Il telefono ha scansionato il codice: attende conferma.
    Scanned(String),
    Token(String),
    Failed(String),
}

pub fn spawn(tx: UnboundedSender<AppEvent>) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = run(&tx).await {
            info!("login QR terminato: {e:#}");
            let _ = tx.send(AppEvent::Auth(AuthEvent::Failed(format!("{e:#}"))));
        }
    })
}

fn emit(tx: &UnboundedSender<AppEvent>, ev: AuthEvent) {
    let _ = tx.send(AppEvent::Auth(ev));
}

fn decrypt(key: &RsaPrivateKey, b64: &str) -> Result<Vec<u8>> {
    let data = STANDARD.decode(b64).context("base64 non valido")?;
    key.decrypt(Oaep::new::<Sha256>(), &data)
        .context("decifratura RSA non riuscita")
}

fn field<'a>(v: &'a Value, name: &str) -> Result<&'a str> {
    v[name]
        .as_str()
        .with_context(|| format!("campo `{name}` mancante"))
}

async fn run(tx: &UnboundedSender<AppEvent>) -> Result<()> {
    emit(
        tx,
        AuthEvent::Progress("Generazione delle chiavi di cifratura…".into()),
    );
    let key = tokio::task::spawn_blocking(|| RsaPrivateKey::new(&mut OsRng, 2048)).await??;
    let public_key = RsaPublicKey::from(&key).to_public_key_der()?;
    let public_b64 = STANDARD.encode(public_key.as_bytes());

    emit(tx, AuthEvent::Progress("Connessione a Discord…".into()));
    let mut request = GATEWAY_URL.into_client_request()?;
    request
        .headers_mut()
        .insert("Origin", HeaderValue::from_static("https://discord.com"));
    let (ws, _) = timeout(Duration::from_secs(20), connect_async(request))
        .await
        .context("timeout di connessione")??;
    let (mut sink, mut stream) = ws.split();
    let mut heartbeat: Option<Interval> = None;

    loop {
        tokio::select! {
            _ = tick_opt(&mut heartbeat) => {
                send_json(&mut sink, &json!({ "op": "heartbeat" })).await?;
            }
            msg = stream.next() => {
                let text = match msg {
                    None => bail!("Il codice QR è scaduto"),
                    Some(Err(e)) => return Err(e.into()),
                    Some(Ok(WsMessage::Text(t))) => t,
                    Some(Ok(WsMessage::Close(_))) => bail!("Il codice QR è scaduto"),
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
                        // Formato: "id:discriminator:avatar:username"
                        let payload = decrypt(&key, field(&v, "encrypted_user_payload")?)?;
                        let payload = String::from_utf8_lossy(&payload);
                        let username = payload.splitn(4, ':').nth(3).unwrap_or("?").to_string();
                        emit(tx, AuthEvent::Scanned(username));
                    }
                    "pending_login" => {
                        emit(tx, AuthEvent::Progress("Accesso in corso…".into()));
                        let token = exchange_ticket(field(&v, "ticket")?, &key).await?;
                        emit(tx, AuthEvent::Token(token));
                        return Ok(());
                    }
                    "cancel" => bail!("Accesso annullato dal telefono"),
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
        .build()?;
    let resp = http
        .post(LOGIN_URL)
        .json(&json!({ "ticket": ticket }))
        .send()
        .await?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);

    if body.get("captcha_key").is_some() || body.get("captcha_sitekey").is_some() {
        bail!("Discord chiede un captcha per questo accesso: usa il login con token (Tab)");
    }
    if !status.is_success() {
        bail!("Scambio del ticket non riuscito (HTTP {})", status.as_u16());
    }
    let token = decrypt(key, field(&body, "encrypted_token")?)?;
    Ok(String::from_utf8(token)?)
}
