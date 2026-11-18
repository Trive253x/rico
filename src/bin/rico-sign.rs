//! `rico-sign`: the release signing tool, used only on the owner's PC.
//!
//! ```text
//! rico-sign keygen                  everyday key, saved in the system keychain
//! rico-sign keygen --spare FILE     spare key, written to FILE (keep it offline)
//! rico-sign public                  the everyday public key (for update::PUBLIC_KEYS)
//! rico-sign manifest INSTALLER --version V [--channel stable|candidate]
//!           [--notes URL] [--insecure-below V] --out FILE
//! rico-sign sign MANIFEST [--spare FILE]     writes MANIFEST.sig
//! rico-sign verify MANIFEST                  checks MANIFEST.sig with Rico's keys
//! ```
//!
//! The private key never goes in a file of the repository, in the output or
//! in the log: the everyday one stays in the system keychain, the spare one
//! in the file the owner chose (to move offline). See docs/update-design.md.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
use rico::update;
use sha2::{Digest, Sha256};

const KEYCHAIN_SERVICE: &str = "rico-release-signing";
const KEYCHAIN_USER: &str = "everyday";

fn main() {
    if let Err(e) = run(std::env::args().skip(1).collect()) {
        eprintln!("rico-sign: {e:#}");
        std::process::exit(1);
    }
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn run(args: Vec<String>) -> Result<()> {
    let mut it = args.into_iter();
    let cmd = it.next().unwrap_or_default();
    let rest: Vec<String> = it.collect();
    match cmd.as_str() {
        "keygen" => keygen(option(&rest, "--spare").map(PathBuf::from)),
        "public" => {
            let key = everyday_key()?;
            print_public(&key);
            Ok(())
        }
        "manifest" => manifest(&rest),
        "sign" => {
            let path = rest.first().context("which manifest?")?;
            let key = match option(&rest, "--spare") {
                Some(file) => spare_key(Path::new(&file))?,
                None => everyday_key()?,
            };
            sign(Path::new(path), &key)
        }
        "verify" => {
            let path = rest.first().context("which manifest?")?;
            verify(Path::new(path))
        }
        _ => {
            eprintln!("usage: rico-sign keygen [--spare FILE] | public | manifest | sign | verify");
            std::process::exit(2);
        }
    }
}

/// The value after `name` in the arguments.
fn option(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn new_key() -> SigningKey {
    let mut seed = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
    let key = SigningKey::from_bytes(&seed);
    seed.fill(0);
    key
}

fn keygen(spare: Option<PathBuf>) -> Result<()> {
    let key = new_key();
    if let Some(path) = spare {
        if path.exists() {
            bail!("{} already exists: not overwritten", path.display());
        }
        std::fs::write(&path, b64().encode(key.to_bytes()))?;
        eprintln!(
            "Spare key written to {}: move it offline (a USB stick in a drawer).",
            path.display()
        );
    } else {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER)?;
        if entry.get_password().is_ok() {
            bail!("an everyday key already exists in the keychain: not replaced");
        }
        entry.set_password(&b64().encode(key.to_bytes()))?;
        eprintln!("Everyday key saved in the system keychain.");
    }
    print_public(&key);
    Ok(())
}

fn decode_key(text: &str) -> Result<SigningKey> {
    let bytes = b64().decode(text.trim()).context("not a key")?;
    let seed: [u8; 32] = bytes.as_slice().try_into().context("not a key")?;
    Ok(SigningKey::from_bytes(&seed))
}

fn everyday_key() -> Result<SigningKey> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER)?;
    decode_key(
        &entry
            .get_password()
            .context("no everyday key: run `rico-sign keygen`")?,
    )
}

fn spare_key(path: &Path) -> Result<SigningKey> {
    decode_key(&std::fs::read_to_string(path)?)
}

/// The public key, as base64 and as the line for `update::PUBLIC_KEYS`.
fn print_public(key: &SigningKey) {
    let public = key.verifying_key().to_bytes();
    println!("public key: {}", b64().encode(public));
    let bytes: Vec<String> = public.iter().map(|b| format!("0x{b:02x}")).collect();
    println!("for update::PUBLIC_KEYS: [{}],", bytes.join(", "));
}

fn manifest(args: &[String]) -> Result<()> {
    let installer = args.first().context("which installer?")?;
    let version = option(args, "--version").context("--version")?;
    semver::Version::parse(&version).context("--version is not a version")?;
    let channel = option(args, "--channel").unwrap_or_else(|| "stable".into());
    let out = option(args, "--out").context("--out")?;
    let bytes = std::fs::read(installer)?;
    let file = Path::new(installer)
        .file_name()
        .and_then(|n| n.to_str())
        .context("installer name")?
        .to_string();
    let hash = Sha256::digest(&bytes);
    let sha256 = hash.iter().fold(String::with_capacity(64), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{b:02x}");
        acc
    });
    let mut m = serde_json::json!({
        "version": version,
        "channel": channel,
        "file": file,
        "size": bytes.len(),
        "sha256": sha256,
        "signed_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    });
    if let Some(n) = option(args, "--notes") {
        m["notes_url"] = serde_json::json!(n);
    }
    if let Some(v) = option(args, "--insecure-below") {
        m["insecure_below"] = serde_json::json!(v);
    }
    std::fs::write(&out, serde_json::to_vec_pretty(&m)?)?;
    eprintln!("{out} written ({} bytes of installer).", bytes.len());
    Ok(())
}

fn sign(path: &Path, key: &SigningKey) -> Result<()> {
    let bytes = std::fs::read(path)?;
    let sig = key.sign(&bytes);
    // Checked with the same rules as Rico before writing it.
    let public = key.verifying_key().to_bytes();
    let text = b64().encode(sig.to_bytes());
    update::verify(&bytes, text.as_bytes(), &[public]).map_err(|r| anyhow::anyhow!("{r}"))?;
    let out = PathBuf::from(format!("{}.sig", path.display()));
    std::fs::write(&out, format!("{text}\n"))?;
    eprintln!("{} written.", out.display());
    if !update::PUBLIC_KEYS.contains(&public) {
        eprintln!("note: this key is not (yet) in update::PUBLIC_KEYS of this build.");
    }
    Ok(())
}

fn verify(path: &Path) -> Result<()> {
    let bytes = std::fs::read(path)?;
    let sig = std::fs::read(format!("{}.sig", path.display()))?;
    let v = update::verify(&bytes, &sig, update::PUBLIC_KEYS).map_err(|r| anyhow::anyhow!("{r}"))?;
    println!(
        "OK: Rico {} ({}), signed with one of Rico's keys.",
        v.manifest.version, v.manifest.channel
    );
    Ok(())
}
