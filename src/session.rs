//! Login steps shared by the GUI and the terminal: checking a pasted token,
//! the email and password form, what each answer of Discord means, and the
//! REST client of a session.
//!
//! No interface and no runtime here: each interface runs the futures its
//! own way and shows the texts returned. Tokens never end up in a `Debug`
//! output or in the texts.

use crate::discord::models::User;
use crate::discord::password_login::{self, Answer, TwoFactor};
use crate::discord::rest::{self, Rest, Unauthorized};
use crate::i18n::t;

/// A pasted token, cleaned, with the client to check it; otherwise the
/// text to show.
pub fn prepare_token(raw: &str) -> Result<(String, Rest), String> {
    let token = rest::clean_token(raw)?;
    let client = Rest::new(&token).map_err(|e| tf!("Invalid token: {e:#}", "Token non valido: {e:#}"))?;
    Ok((token, client))
}

/// Checks a token with Discord: who it belongs to, or the text to show.
pub async fn verify_token(client: Rest, token: String) -> Result<(String, User), String> {
    match client.me().await {
        Ok(user) => Ok((token, user)),
        Err(e) if e.is::<Unauthorized>() => {
            Err(t("Invalid or expired token.", "Token non valido o scaduto.").to_string())
        }
        Err(e) => Err(tf!("Verification failed: {e:#}", "Verifica non riuscita: {e:#}")),
    }
}

/// The REST client of a session that starts with this token.
pub fn client(token: &str) -> Result<Rest, String> {
    Rest::new(token).map_err(|e| tf!("Unusable token: {e:#}", "Token non utilizzabile: {e:#}"))
}

/// Saves the token in the system keychain; the text to show if it could not.
pub fn save_token(token: &str) -> Option<String> {
    crate::config::save_token(token)
        .err()
        .map(|e| tf!("Token not saved: {e:#}", "Token non salvato: {e:#}"))
}

/// The email to send, trimmed, when both fields are filled in.
pub fn credentials(email: &str, password: &str) -> Result<String, String> {
    let email = email.trim();
    if email.is_empty() || password.is_empty() {
        return Err(t(
            "Write the email and the password.",
            "Scrivi l'email e la password.",
        )
        .into());
    }
    Ok(email.to_string())
}

/// What an answer of the email login means for the form. Whatever it is,
/// the password and the code typed are no longer needed.
pub enum Next {
    /// Signed in: start the session with this token.
    SignedIn(String),
    /// Ask the code of the authenticator app.
    AskCode(TwoFactor),
    /// Authorized from the email: sign in again from the start.
    Notice(String),
    /// Show this error. `forget_code`: the two-factor step is over (a
    /// wrong code keeps it, so the code can be written again).
    Error { message: String, forget_code: bool },
}

pub fn next_step(answer: Answer) -> Next {
    match answer {
        Answer::Token(token) => Next::SignedIn(token),
        Answer::TwoFactor(m) if m.app => Next::AskCode(m),
        Answer::TwoFactor(_) => Next::Error {
            message: t(
                "This account confirms logins by SMS or security key, which Rico does not handle: use the QR code.",
                "Questo account conferma gli accessi con SMS o chiave di sicurezza, che Rico non gestisce: usa il QR code.",
            )
            .into(),
            forget_code: true,
        },
        Answer::NewLocation => Next::Notice(password_login::new_location_notice().into()),
        Answer::Captcha => Next::Error {
            message: password_login::captcha_notice().into(),
            forget_code: true,
        },
        Answer::Refused(reason) => Next::Error {
            message: reason,
            forget_code: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_and_answers() {
        assert_eq!(credentials("  a@b.c ", "pw").as_deref(), Ok("a@b.c"));
        assert!(credentials(" ", "pw").is_err());
        assert!(credentials("a@b.c", "").is_err());
        assert!(prepare_token("").is_err());
        assert!(prepare_token("Bot abc").is_err());
        assert!(matches!(next_step(Answer::Token("x".into())), Next::SignedIn(t) if t == "x"));
        assert!(matches!(next_step(Answer::NewLocation), Next::Notice(_)));
        assert!(matches!(
            next_step(Answer::Captcha),
            Next::Error {
                forget_code: true,
                ..
            }
        ));
        assert!(matches!(
            next_step(Answer::Refused("no".into())),
            Next::Error {
                forget_code: false,
                ..
            }
        ));
    }
}
