//! Login with email (or phone) and password, with the two-factor code when
//! the account has one, as Discord's web client does.
//!
//! The password stays in memory only for the request: it is sent only to
//! `discord.com` over HTTPS (no redirects followed), never saved and never
//! written to the log. Only the token that comes back is kept, in the
//! system's keychain like the other logins.
//!
//! What Discord can answer instead of a token, besides a refusal:
//! - the two-factor code is needed (a ticket valid for a few minutes);
//! - a new login location: Discord emails the owner a link to authorize it,
//!   after which the login has to be done again;
//! - a captcha, which ferrocord cannot show.

use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};
use tracing::info;

use super::USER_AGENT;
use crate::i18n::t;

const LOGIN_URL: &str = "https://discord.com/api/v9/auth/login";
const TOTP_URL: &str = "https://discord.com/api/v9/auth/mfa/totp";

/// Error code Discord uses when a login comes from a place it does not know
/// yet and it sent the authorization email.
const NEW_LOCATION: &str = "ACCOUNT_LOGIN_VERIFICATION_EMAIL";

/// What Discord answered to a login (or to the two-factor code).
#[derive(Clone, PartialEq, Eq)]
pub enum Answer {
    Token(String),
    /// The account has two-factor authentication: the code goes with the ticket.
    TwoFactor(TwoFactor),
    /// Discord sent an email to authorize this new location: once it is
    /// authorized the login has to be done again.
    NewLocation,
    /// Discord wants a captcha, which ferrocord cannot show.
    Captcha,
    /// Refused, with the reason to show.
    Refused(String),
}

#[derive(Clone, PartialEq, Eq)]
pub struct TwoFactor {
    ticket: String,
    /// The account has an authenticator app (the only kind of code handled here).
    pub app: bool,
}

// Token and ticket never end up in a `{:?}` (the interface's messages are Debug).
impl std::fmt::Debug for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Token(_) => f.write_str("Token(…)"),
            Self::TwoFactor(m) => write!(f, "TwoFactor {{ app: {} }}", m.app),
            Self::NewLocation => f.write_str("NewLocation"),
            Self::Captcha => f.write_str("Captcha"),
            Self::Refused(r) => write!(f, "Refused({r:?})"),
        }
    }
}

impl std::fmt::Debug for TwoFactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TwoFactor {{ app: {} }}", self.app)
    }
}

impl TwoFactor {
    #[cfg(test)]
    pub fn for_tests(app: bool) -> Self {
        Self {
            ticket: "ticket".into(),
            app,
        }
    }
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(20))
        // The answer holds the token: only from Discord's API.
        .redirect(reqwest::redirect::Policy::none())
        .https_only(true)
        .build()?)
}

async fn post(url: &str, body: &Value) -> Result<(u16, Value)> {
    let resp = client()?.post(url).json(body).send().await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    Ok((status, body))
}

/// First step: email (or phone number) and password.
pub async fn login(login: &str, password: &str) -> Result<Answer, String> {
    let body = json!({
        "login": login.trim(),
        "password": password,
        "undelete": false,
        "login_source": null,
        "gift_code_sku_id": null,
    });
    let answer = match post(LOGIN_URL, &body).await {
        Ok((status, body)) => classify(status, &body),
        Err(e) => return Err(network_error(&e)),
    };
    info!("password login: {}", kind(&answer));
    Ok(answer)
}

/// Second step, when asked: the code of the authenticator app.
pub async fn two_factor(challenge: &TwoFactor, code: &str) -> Result<Answer, String> {
    let code: String = code.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
    let body = json!({
        "code": code,
        "ticket": challenge.ticket,
        "login_source": null,
        "gift_code_sku_id": null,
    });
    let answer = match post(TOTP_URL, &body).await {
        Ok((status, body)) => classify(status, &body),
        Err(e) => return Err(network_error(&e)),
    };
    info!("two-factor code: {}", kind(&answer));
    Ok(answer)
}

fn network_error(e: &anyhow::Error) -> String {
    info!("password login: no answer from Discord");
    tf!("Discord did not answer: {e:#}", "Discord non ha risposto: {e:#}")
}

/// For the log: only the kind of answer, never its content.
fn kind(a: &Answer) -> &'static str {
    match a {
        Answer::Token(_) => "signed in",
        Answer::TwoFactor(_) => "two-factor code needed",
        Answer::NewLocation => "new location to authorize by email",
        Answer::Captcha => "captcha asked",
        Answer::Refused(_) => "refused",
    }
}

/// Every `{code, message}` in Discord's error tree (`errors.login._errors[]`…).
fn error_codes(v: &Value, out: &mut Vec<(String, String)>) {
    match v {
        Value::Object(map) => {
            if let Some(Value::Array(list)) = map.get("_errors") {
                for e in list {
                    out.push((
                        e["code"].as_str().unwrap_or_default().to_string(),
                        e["message"].as_str().unwrap_or_default().to_string(),
                    ));
                }
            }
            for (k, child) in map {
                if k != "_errors" {
                    error_codes(child, out);
                }
            }
        }
        Value::Array(list) => list.iter().for_each(|c| error_codes(c, out)),
        _ => {}
    }
}

/// Reads Discord's answer (pure: tested without the network).
pub fn classify(status: u16, body: &Value) -> Answer {
    if let Some(token) = body["token"].as_str().filter(|t| !t.is_empty()) {
        return Answer::Token(token.to_string());
    }
    if body["mfa"].as_bool() == Some(true) {
        if let Some(ticket) = body["ticket"].as_str() {
            return Answer::TwoFactor(TwoFactor {
                ticket: ticket.to_string(),
                app: body["totp"].as_bool().unwrap_or(false),
            });
        }
    }
    if body.get("captcha_key").is_some() || body.get("captcha_sitekey").is_some() {
        return Answer::Captcha;
    }
    let mut errors = Vec::new();
    error_codes(&body["errors"], &mut errors);
    if errors.iter().any(|(code, _)| code == NEW_LOCATION) {
        return Answer::NewLocation;
    }
    if status == 429 {
        let wait = body["retry_after"].as_f64().unwrap_or(0.0).ceil().max(1.0) as u64;
        return Answer::Refused(tf!(
            "Too many attempts: try again in {wait} s.",
            "Troppi tentativi: riprova tra {wait} s."
        ));
    }
    if errors
        .iter()
        .any(|(code, _)| code == "INVALID_LOGIN" || code == "INVALID_PASSWORD")
    {
        return Answer::Refused(t("Wrong email or password.", "Email o password non corretti.").into());
    }
    // 60008: wrong two-factor code.
    if body["code"].as_u64() == Some(60008) {
        return Answer::Refused(t("Wrong code.", "Codice non corretto.").into());
    }
    // Discord's own explanation, when there is one.
    let reason = errors
        .iter()
        .map(|(_, m)| m.as_str())
        .find(|m| !m.is_empty())
        .or_else(|| body["message"].as_str().filter(|m| !m.is_empty()));
    Answer::Refused(match reason {
        Some(r) => r.chars().filter(|c| !c.is_control()).take(200).collect(),
        None => tf!(
            "Login refused (HTTP {status}).",
            "Accesso rifiutato (HTTP {status})."
        ),
    })
}

/// Text for a new location to authorize, shared by every login.
pub fn new_location_notice() -> &'static str {
    t(
        "Discord sent you an email to authorize this new login location. Approve it from the email, then sign in again.",
        "Discord ti ha mandato una mail per autorizzare l'accesso da questa nuova posizione. Approvalo dalla mail, poi rifai il login.",
    )
}

/// Text for a captcha, shared by every login.
pub fn captcha_notice() -> &'static str {
    t(
        "Discord asks for a captcha, which ferrocord cannot show. Sign in once on discord.com in the browser, from this network, then try again here (or use the token).",
        "Discord chiede un captcha, che ferrocord non può mostrare. Accedi una volta su discord.com dal browser, da questa rete, poi riprova qui (oppure usa il token).",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_signs_in() {
        let a = classify(200, &json!({"user_id": "1", "token": "abc", "user_settings": {}}));
        assert_eq!(a, Answer::Token("abc".into()));
        // Never shown by Debug.
        assert!(!format!("{a:?}").contains("abc"));
    }

    #[test]
    fn two_factor_keeps_the_ticket() {
        let a = classify(
            200,
            &json!({"mfa": true, "ticket": "t1", "totp": true, "sms": false}),
        );
        let Answer::TwoFactor(m) = &a else { panic!("{a:?}") };
        assert!(m.app);
        assert_eq!(m.ticket, "t1");
        assert!(!format!("{a:?}").contains("t1"));
        let a = classify(
            200,
            &json!({"mfa": true, "ticket": "t2", "totp": false, "sms": true}),
        );
        assert!(matches!(a, Answer::TwoFactor(TwoFactor { app: false, .. })));
    }

    #[test]
    fn a_new_location_asks_to_authorize_it() {
        let body = json!({
            "code": 50035,
            "errors": {"login": {"_errors": [{
                "code": "ACCOUNT_LOGIN_VERIFICATION_EMAIL",
                "message": "New login location detected, please check your e-mail."
            }]}},
            "message": "Invalid Form Body"
        });
        assert_eq!(classify(400, &body), Answer::NewLocation);
    }

    #[test]
    fn captcha_and_refusals() {
        let captcha = json!({"captcha_key": ["captcha-required"], "captcha_sitekey": "x", "captcha_service": "hcaptcha"});
        assert_eq!(classify(400, &captcha), Answer::Captcha);

        let wrong = json!({"code": 50035, "errors": {
            "login": {"_errors": [{"code": "INVALID_LOGIN", "message": "Login or password is invalid."}]},
            "password": {"_errors": [{"code": "INVALID_LOGIN", "message": "Login or password is invalid."}]}
        }});
        assert!(matches!(classify(400, &wrong), Answer::Refused(_)));

        let code = json!({"code": 60008, "message": "Invalid two-factor code"});
        assert!(matches!(classify(400, &code), Answer::Refused(_)));

        let slow = json!({"retry_after": 12.3, "message": "You are being rate limited."});
        let Answer::Refused(r) = classify(429, &slow) else {
            panic!()
        };
        assert!(r.contains("13"), "{r}");

        let other = json!({"message": "Account disabled\u{7}"});
        assert_eq!(classify(403, &other), Answer::Refused("Account disabled".into()));
        assert!(matches!(classify(500, &Value::Null), Answer::Refused(_)));
    }
}
