//! Links from messages opened in the default browser, after the user
//! confirmed them.
//!
//! A link comes from others: only `http` and `https`, no user name or
//! password in it, never a local or private address (the browser would be
//! used to reach a service on this computer or the local network), at most
//! [`MAX_LEN`] characters. The address handed to the system is the one
//! `url` rewrites (spaces, quotes and control characters percent-encoded,
//! international names as `xn--`), passed as one argument, never through a
//! shell.

use std::net::{Ipv4Addr, Ipv6Addr};

use reqwest::Url;

/// Longest link opened.
pub const MAX_LEN: usize = 2048;

/// A link that can be opened: its address as it will be opened, and the
/// site to show the user before opening it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    pub host: String,
}

/// The link, if it may be opened.
pub fn checked(raw: &str) -> Option<Link> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > MAX_LEN {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    // `url` writes IP addresses in their usual form (`0x7f.1` → `127.0.0.1`).
    let name = url.host_str()?.to_ascii_lowercase();
    let host = if let Some(v6) = name.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        let ip: Ipv6Addr = v6.parse().ok()?;
        if !public_v6(ip) {
            return None;
        }
        name
    } else if let Ok(ip) = name.parse::<Ipv4Addr>() {
        if !public_v4(ip) {
            return None;
        }
        name
    } else {
        let d = name.trim_end_matches('.');
        let local = d == "localhost"
            || d.ends_with(".localhost")
            || d.ends_with(".local")
            || d.ends_with(".internal")
            || d.ends_with(".lan")
            || d.ends_with(".home.arpa")
            || !d.contains('.');
        if local {
            return None;
        }
        d.to_string()
    };
    let url = String::from(url);
    (url.len() <= MAX_LEN && url.chars().all(|c| c.is_ascii_graphic())).then_some(Link { url, host })
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        // Shared address space (carrier-grade NAT) and benchmarking.
        || (a == 100 && (64..128).contains(&b))
        || (a == 198 && (18..20).contains(&b))
        || a >= 240)
}

fn public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return public_v4(v4);
    }
    let first = ip.segments()[0];
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        // Unique local (fc00::/7) and link-local (fe80::/10).
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80)
}

/// Opens a checked link in the default browser. Blocking for a moment (a
/// program is started): call it off the interface thread.
pub fn open(link: &Link) -> std::io::Result<()> {
    use std::process::Command;
    // Checked again: this is the last step before another program.
    let Some(link) = checked(&link.url) else {
        return Err(std::io::Error::other("link not allowed"));
    };
    let mut child = if cfg!(windows) {
        // The same handler as a click on a link in Windows; the address is
        // one argument, with no spaces or quotes in it.
        Command::new("rundll32")
            .arg("url.dll,FileProtocolHandler")
            .arg(&link.url)
            .spawn()?
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(&link.url).spawn()?
    } else {
        Command::new("xdg-open").arg(&link.url).spawn()?
    };
    // Collected when it ends (it hands the link over and quits).
    std::thread::Builder::new()
        .name("open-link".into())
        .spawn(move || {
            let _ = child.wait();
        })
        .map(|_| ())
}

/// The link, a little shortened for showing (the whole one is opened).
pub fn shown(link: &Link) -> String {
    const SHOWN: usize = 300;
    if link.url.chars().count() <= SHOWN {
        link.url.clone()
    } else {
        let mut s: String = link.url.chars().take(SHOWN).collect();
        s.push('…');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_web_links_are_opened() {
        let ok = |s: &str| checked(s).map(|l| l.host);
        assert_eq!(
            ok("https://blog.example.org/a?b=1#c").as_deref(),
            Some("blog.example.org")
        );
        assert_eq!(ok("http://Example.COM").as_deref(), Some("example.com"));
        assert_eq!(ok("https://93.184.216.34/x").as_deref(), Some("93.184.216.34"));
        for bad in [
            "",
            "javascript:alert(1)",
            "file:///C:/Windows/system32/calc.exe",
            "ms-settings:privacy",
            "ftp://example.com/x",
            "https://user:pass@example.com/",
            "https://user@example.com/",
            "http://localhost:8080/",
            "http://app.localhost/",
            "http://printer.local/",
            "http://router/",
            "http://127.0.0.1/",
            "http://10.0.0.1/",
            "http://192.168.1.1/",
            "http://172.16.0.1/",
            "http://169.254.169.254/latest/meta-data",
            "http://100.64.0.1/",
            "http://0.0.0.0/",
            "http://[::1]/",
            "http://[fe80::1]/",
            "http://[fd00::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://2130706433/",
            "http://0x7f.1/",
        ] {
            assert_eq!(checked(bad), None, "{bad}");
        }
        assert_eq!(
            checked(&format!("https://example.com/{}", "a".repeat(MAX_LEN))),
            None
        );
    }

    #[test]
    fn what_is_opened_is_rewritten_safely() {
        // Spaces, quotes and control characters cannot reach the command.
        let l = checked("https://example.com/a b\"c\t,d").unwrap();
        assert!(l.url.chars().all(|c| c.is_ascii_graphic()), "{}", l.url);
        assert!(!l.url.contains('"') && !l.url.contains(' '));
        // International names as `xn--`: a look-alike shows as what it is.
        let l = checked("https://еxample.com/").unwrap();
        assert!(l.host.starts_with("xn--"), "{}", l.host);
        let long = checked(&format!("https://example.com/{}", "a".repeat(400))).unwrap();
        assert!(shown(&long).ends_with('…'));
    }
}
