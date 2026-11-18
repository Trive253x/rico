//! Discord's captcha, solved by the user in their browser.
//!
//! Some logins answer `400` with `captcha_key` instead of a token. The
//! challenge is an hCaptcha widget, which needs a web page: Rico serves one
//! on `127.0.0.1` (random port, random 128-bit path, for this one challenge
//! only), the user opens it in their browser and solves it, the page posts
//! the answer back, and the request is sent again with it in the
//! `X-Captcha-Key` header (plus `X-Captcha-Session-Id` and
//! `X-Captcha-Rqtoken` when Discord gave them), as Discord's own clients do.
//!
//! The local server answers only that path, only with a `Host` of
//! `127.0.0.1:<port>` (no DNS rebinding), reads at most 16 KB per request,
//! takes one answer and then closes; it gives up after [`WAIT`]. Nothing of
//! the challenge or the answer is written to the log, only which fields
//! Discord sent.

use std::fmt::Write as _;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use rand::RngCore;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tracing::info;

/// How long the page waits for the captcha to be solved.
pub const WAIT: Duration = Duration::from_secs(5 * 60);
/// Largest request the local page accepts.
const MAX_REQUEST: usize = 16 * 1024;
/// Largest answer (hCaptcha's are a few KB).
const MAX_ANSWER: usize = 12 * 1024;

/// What Discord sent with a captcha request.
#[derive(Clone, PartialEq, Eq)]
pub struct Challenge {
    pub service: String,
    pub sitekey: String,
    session_id: Option<String>,
    rqdata: Option<String>,
    rqtoken: Option<String>,
}

// The values are only for Discord: never in a `{:?}` or the log.
impl std::fmt::Debug for Challenge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.describe())
    }
}

fn short(v: Option<&str>, max: usize) -> Option<String> {
    v.filter(|s| !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control))
        .map(str::to_string)
}

impl Challenge {
    /// The challenge in a Discord answer, if there is one (and it can be shown).
    pub fn from_body(body: &Value) -> Option<Self> {
        body.get("captcha_key")?;
        let service = body["captcha_service"].as_str().unwrap_or("hcaptcha").to_string();
        let sitekey = body["captcha_sitekey"]
            .as_str()
            .filter(|k| {
                !k.is_empty() && k.len() <= 64 && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            })?
            .to_string();
        Some(Self {
            service,
            sitekey,
            session_id: short(body["captcha_session_id"].as_str(), 256),
            rqdata: short(body["captcha_rqdata"].as_str(), 16 * 1024),
            rqtoken: short(body["captcha_rqtoken"].as_str(), 16 * 1024),
        })
    }

    /// Only hCaptcha can be shown.
    pub fn supported(&self) -> bool {
        self.service == "hcaptcha"
    }

    /// For the log: the service and which fields came, never their values.
    pub fn describe(&self) -> String {
        format!(
            "Challenge {{ service: {}, session_id: {}, rqdata: {}, rqtoken: {} }}",
            self.service,
            self.session_id.is_some(),
            self.rqdata.is_some(),
            self.rqtoken.is_some()
        )
    }

    /// The headers that carry the answer when the request is sent again.
    pub fn headers(&self, answer: &str) -> Vec<(&'static str, String)> {
        let mut h = vec![("X-Captcha-Key", answer.to_string())];
        if let Some(s) = &self.session_id {
            h.push(("X-Captcha-Session-Id", s.clone()));
        }
        if let Some(r) = &self.rqtoken {
            h.push(("X-Captcha-Rqtoken", r.clone()));
        }
        h
    }
}

/// A JavaScript string literal that cannot end the `<script>` it is in.
fn js_string(s: &str) -> String {
    serde_json::to_string(s)
        .unwrap_or_else(|_| "\"\"".into())
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
}

fn page(challenge: &Challenge, nonce: &str) -> String {
    let rqdata = challenge
        .rqdata
        .as_deref()
        .map_or_else(|| "null".to_string(), js_string);
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>Rico — captcha</title>
<style>
body{{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;background:#1e1f24;color:#e6e6e6;font:15px/1.5 system-ui,sans-serif}}
main{{max-width:420px;padding:24px;text-align:center}} h1{{font-size:22px;margin:0 0 8px}} p{{color:#b8bcc6}}
#c{{display:inline-block;margin:16px 0}} #s{{color:#9ece6a;font-weight:600}}
</style>
<script nonce="{nonce}">
var SITEKEY = {sitekey};
var RQDATA = {rqdata};
function ready() {{
  var id = hcaptcha.render("c", {{ sitekey: SITEKEY, theme: "dark", callback: done }});
  if (RQDATA) {{ hcaptcha.setData(id, {{ rqdata: RQDATA }}); }}
}}
function done(answer) {{
  fetch(location.pathname, {{ method: "POST", body: answer }}).then(function (r) {{
    document.getElementById("s").textContent = r.ok
      ? "Done. Go back to Rico — you can close this page."
      : "Rico did not accept the answer: try again from Rico.";
  }});
}}
</script>
<script nonce="{nonce}" src="https://js.hcaptcha.com/1/api.js?render=explicit&onload=ready" async defer></script>
</head><body><main>
<h1>Rico</h1>
<p>Discord asks to confirm that you are a person before signing in. Solve the captcha below, then go back to Rico.</p>
<div id="c"></div>
<p id="s"></p>
</main></body></html>
"#,
        sitekey = js_string(&challenge.sitekey),
    )
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf.iter().fold(String::with_capacity(bytes * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The local page of one challenge.
pub struct Served {
    /// To open in the browser.
    pub url: String,
    answer: oneshot::Receiver<String>,
}

impl Served {
    /// Waits for the answer (at most [`WAIT`]).
    pub async fn answer(self) -> Result<String> {
        match tokio::time::timeout(WAIT, self.answer).await {
            Ok(Ok(a)) => Ok(a),
            Ok(Err(_)) => bail!("the captcha page closed"),
            Err(_) => bail!(crate::i18n::t(
                "The captcha was not solved in time.",
                "Il captcha non è stato risolto in tempo."
            )),
        }
    }
}

/// Starts the local page for `challenge`.
pub async fn serve(challenge: Challenge) -> Result<Served> {
    if !challenge.supported() {
        bail!("captcha service {} not supported", challenge.service);
    }
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("local captcha page")?;
    let port = listener.local_addr()?.port();
    let path = format!("/captcha/{}", random_hex(16));
    let url = format!("http://127.0.0.1:{port}{path}");
    let (tx, rx) = oneshot::channel();
    info!("captcha: local page open ({})", challenge.describe());
    tokio::spawn(async move {
        let mut tx = Some(tx);
        let host = format!("127.0.0.1:{port}");
        let run = async {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                match handle(stream, &host, &path, &challenge).await {
                    Ok(Some(answer)) => {
                        if let Some(tx) = tx.take() {
                            let _ = tx.send(answer);
                        }
                        break;
                    }
                    Ok(None) => {}
                    Err(e) => tracing::debug!("captcha page: {e:#}"),
                }
                if tx.as_ref().is_some_and(oneshot::Sender::is_closed) {
                    break;
                }
            }
        };
        let _ = tokio::time::timeout(WAIT, run).await;
        info!("captcha: local page closed");
    });
    Ok(Served { url, answer: rx })
}

/// One request: the page, the answer (returned), or 404.
async fn handle(
    mut stream: TcpStream,
    host: &str,
    path: &str,
    challenge: &Challenge,
) -> Result<Option<String>> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let read = async {
        loop {
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.len() > MAX_REQUEST {
                bail!("request too big");
            }
            if let Some(end) = find(&buf, b"\r\n\r\n") {
                let length = content_length(&buf[..end]);
                if buf.len() >= end + 4 + length {
                    break;
                }
            }
        }
        Ok::<_, anyhow::Error>(())
    };
    tokio::time::timeout(Duration::from_secs(10), read)
        .await
        .context("slow request")??;
    let request = parse(&buf);
    let reply = match request {
        Some(r) if r.host == host && r.path == path && r.method == "GET" => {
            let nonce = random_hex(16);
            let csp = format!(
                "default-src 'none'; script-src 'nonce-{nonce}' https://hcaptcha.com https://*.hcaptcha.com; \
                 frame-src https://hcaptcha.com https://*.hcaptcha.com; style-src 'unsafe-inline' \
                 https://hcaptcha.com https://*.hcaptcha.com; connect-src 'self' https://hcaptcha.com \
                 https://*.hcaptcha.com; img-src https://*.hcaptcha.com data:; base-uri 'none'; form-action 'none'"
            );
            let body = page(challenge, &nonce);
            response(
                "200 OK",
                &[
                    ("Content-Type", "text/html; charset=utf-8"),
                    ("Content-Security-Policy", &csp),
                    ("Cache-Control", "no-store"),
                    ("Referrer-Policy", "no-referrer"),
                ],
                &body,
            )
        }
        Some(r) if r.host == host && r.path == path && r.method == "POST" => {
            let answer = r.body.trim();
            if answer.is_empty()
                || answer.len() > MAX_ANSWER
                || !answer
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
            {
                stream
                    .write_all(response("400 Bad Request", &[], "").as_bytes())
                    .await?;
                return Ok(None);
            }
            stream
                .write_all(response("204 No Content", &[("Cache-Control", "no-store")], "").as_bytes())
                .await?;
            return Ok(Some(answer.to_string()));
        }
        _ => response("404 Not Found", &[], ""),
    };
    stream.write_all(reply.as_bytes()).await?;
    Ok(None)
}

fn response(status: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut s = format!(
        "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        let _ = write!(s, "{k}: {v}\r\n");
    }
    s.push_str("X-Content-Type-Options: nosniff\r\n\r\n");
    s.push_str(body);
    s
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn content_length(head: &[u8]) -> usize {
    String::from_utf8_lossy(head)
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse().ok())
                .flatten()
        })
        .unwrap_or(0)
        .min(MAX_REQUEST)
}

struct Request {
    method: String,
    path: String,
    host: String,
    body: String,
}

fn parse(buf: &[u8]) -> Option<Request> {
    let end = find(buf, b"\r\n\r\n")?;
    let head = std::str::from_utf8(&buf[..end]).ok()?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let path = first.next()?.to_string();
    let host = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("host"))
        .map(|(_, v)| v.trim().to_string())?;
    let body = String::from_utf8_lossy(buf.get(end + 4..)?).into_owned();
    Some(Request {
        method,
        path,
        host,
        body,
    })
}

/// Opens our local page in the default browser (only that kind of URL; no
/// shell is involved).
pub fn open_in_browser(url: &str) -> Result<()> {
    if !url.starts_with("http://127.0.0.1:") || url.chars().any(|c| c.is_whitespace() || c == '"') {
        bail!("not a captcha page");
    }
    #[cfg(windows)]
    let mut cmd = std::process::Command::new("explorer.exe");
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(url).spawn().context("browser not opened")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body() -> Value {
        json!({
            "captcha_key": ["captcha-required"],
            "captcha_sitekey": "10000000-ffff-ffff-ffff-000000000001",
            "captcha_service": "hcaptcha",
            "captcha_session_id": "SESS-1",
            "captcha_rqdata": "RQ</script><b>",
            "captcha_rqtoken": "TOK-2"
        })
    }

    #[test]
    fn reads_the_challenge_and_hides_it() {
        let c = Challenge::from_body(&body()).unwrap();
        assert!(c.supported());
        let shown = format!("{c:?}");
        assert!(!shown.contains("SESS-1") && !shown.contains("TOK-2") && !shown.contains("RQ<"));
        let h = c.headers("answer");
        assert_eq!(h[0], ("X-Captcha-Key", "answer".into()));
        assert!(h.contains(&("X-Captcha-Session-Id", "SESS-1".into())));
        assert!(h.contains(&("X-Captcha-Rqtoken", "TOK-2".into())));
        // No captcha, or a site key that is not one.
        assert!(Challenge::from_body(&json!({"message": "x"})).is_none());
        assert!(
            Challenge::from_body(&json!({"captcha_key": [], "captcha_sitekey": "\"><script>"})).is_none()
        );
    }

    #[test]
    fn the_page_cannot_be_broken_by_the_challenge() {
        let c = Challenge::from_body(&body()).unwrap();
        let p = page(&c, "n0nce");
        assert!(!p.contains("</script><b>"));
        assert!(p.contains("\\u003c/script\\u003e"));
        assert!(p.contains("nonce=\"n0nce\""));
    }

    #[test]
    fn requests_are_parsed_strictly() {
        let r =
            parse(b"POST /captcha/ab HTTP/1.1\r\nHost: 127.0.0.1:5\r\nContent-Length: 3\r\n\r\nxyz").unwrap();
        assert_eq!(
            (
                r.method.as_str(),
                r.path.as_str(),
                r.host.as_str(),
                r.body.as_str()
            ),
            ("POST", "/captcha/ab", "127.0.0.1:5", "xyz")
        );
        assert!(parse(b"GET / HTTP/1.1\r\n\r\n").is_none(), "no Host");
        assert_eq!(content_length(b"Content-Length: 99999999"), MAX_REQUEST);
    }

    #[tokio::test]
    async fn the_local_page_takes_one_answer() {
        let served = serve(Challenge::from_body(&body()).unwrap()).await.unwrap();
        let url = served.url.clone();
        let rest = url.trim_start_matches("http://");
        let (host, path) = rest.split_at(rest.find('/').unwrap());
        let ask = |req: String| {
            let host = host.to_string();
            async move {
                let mut s = TcpStream::connect(&host).await.unwrap();
                s.write_all(req.as_bytes()).await.unwrap();
                let mut out = String::new();
                s.read_to_string(&mut out).await.unwrap();
                out
            }
        };
        // Another Host (DNS rebinding) or path: nothing.
        let r = ask(format!("GET {path} HTTP/1.1\r\nHost: evil.example\r\n\r\n")).await;
        assert!(r.starts_with("HTTP/1.1 404"), "{r}");
        let r = ask(format!("GET /captcha/0 HTTP/1.1\r\nHost: {host}\r\n\r\n")).await;
        assert!(r.starts_with("HTTP/1.1 404"));
        // The page, with its policy.
        let r = ask(format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n")).await;
        assert!(
            r.starts_with("HTTP/1.1 200") && r.contains("Content-Security-Policy") && r.contains("hcaptcha")
        );
        // A malformed answer is refused, a good one ends it.
        let r = ask(format!(
            "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: 5\r\n\r\na b c"
        ))
        .await;
        assert!(r.starts_with("HTTP/1.1 400"));
        let r = ask(format!(
            "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: 6\r\n\r\nP1.a-b"
        ))
        .await;
        assert!(r.starts_with("HTTP/1.1 204"));
        assert_eq!(served.answer().await.unwrap(), "P1.a-b");
    }
}
