//! Developer keys issued by Lunarstorm, and the certificate that ties a signed
//! `.jbn` package to one.
//!
//! A developer key file is JSON:
//!
//! ```json
//! { "developer": "acme", "expires": 1795000000, "key": "<hex PKCS#8>", "certificate": "<hex>" }
//! ```
//!
//! Decisions:
//! - Two levels of Ed25519: the Lunarstorm root key signs a certificate naming the
//!   developer, the expiry and the developer's public key; the developer key signs
//!   packages. Only the root's public key ships in `lunar` ([`ROOT`]), so a key
//!   that was not issued by Lunarstorm cannot sign.
//! - The certificate covers `"lunarstorm-developer-v1\0" ‖ developer ‖ "\0" ‖
//!   expires (u64 LE) ‖ public key`. A developer id is `[a-z0-9][a-z0-9.-]{0,63}`,
//!   so it holds no NUL and the message has one reading.
//! - `expires` is Unix seconds and is checked when signing. A signed package keeps
//!   its certificate in the manifest metadata ([`METADATA`]), covered by the
//!   package signature, so anyone holding [`ROOT`] can check who signed it.

use ed25519_dalek::pkcs8::DecodePrivateKey;
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use lunarstorm_jbn::{to_hex, Package, Signer};
use serde_json::{json, Value};

/// The Lunarstorm root public key: the one key that issues developer keys. Its
/// private half is kept by Lunarstorm, outside the repository (made with
/// `examples/issue.rs root`, 2026-11-25).
pub const ROOT: [u8; 32] = [
    238, 98, 20, 205, 231, 81, 100, 72, 102, 40, 122, 126, 118, 28, 34, 237, 96, 66, 16, 25, 140, 57, 84, 11, 13, 69, 172, 229, 78, 205,
    53, 217,
];

/// The domain separator of a certificate.
const DOMAIN: &[u8] = b"lunarstorm-developer-v1\0";

/// Manifest metadata keys a signed package carries: developer, expiry, public key
/// and certificate, in that order.
pub const METADATA: [&str; 4] = ["developer", "developer.expires", "developer.key", "developer.certificate"];

/// A developer key whose certificate verified against a root.
pub struct DeveloperKey {
    /// The developer id the certificate names.
    pub developer: String,
    /// Unix seconds after which the key no longer signs.
    pub expires: u64,
    /// The signing key.
    pub signer: Signer,
    /// The root's signature over [`message`].
    pub certificate: [u8; 64],
}

/// Checks a developer id.
///
/// # Arguments
/// * `developer` - the id.
///
/// # Returns
/// Whether it is `[a-z0-9][a-z0-9.-]{0,63}`.
pub fn valid_developer(developer: &str) -> bool {
    let mut bytes = developer.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z' | b'0'..=b'9'))
        && developer.len() <= 64
        && bytes.all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-'))
}

/// Builds the bytes a certificate signs.
///
/// # Arguments
/// * `developer` - the developer id.
/// * `expires` - Unix seconds.
/// * `public` - the developer's public key.
///
/// # Returns
/// The message (see the module documentation).
pub fn message(developer: &str, expires: u64, public: &[u8; 32]) -> Vec<u8> {
    let mut out = DOMAIN.to_vec();
    out.extend_from_slice(developer.as_bytes());
    out.push(0);
    out.extend_from_slice(&expires.to_le_bytes());
    out.extend_from_slice(public);
    out
}

/// Checks a certificate against a root.
///
/// # Arguments
/// * `root` - the root public key.
/// * `developer`, `expires`, `public` - what the certificate claims.
/// * `certificate` - the root's signature.
///
/// # Errors
/// The root key is not a valid point, or the signature does not verify.
pub fn verify(root: &[u8; 32], developer: &str, expires: u64, public: &[u8; 32], certificate: &[u8; 64]) -> Result<(), String> {
    let root = VerifyingKey::from_bytes(root).map_err(|_| String::from("the Lunarstorm root key is not valid"))?;
    root.verify_strict(&message(developer, expires, public), &Signature::from_bytes(certificate))
        .map_err(|_| format!("the key of '{developer}' was not issued by Lunarstorm"))
}

/// Decodes lowercase or uppercase hex.
///
/// # Arguments
/// * `text` - the hex text.
///
/// # Returns
/// The bytes.
///
/// # Errors
/// An odd length or a non-hex digit.
pub fn from_hex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err(String::from("hex has an odd length"));
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2).unwrap_or("x"), 16).map_err(|_| String::from("not hex")))
        .collect()
}

/// Issues a developer key: a new key pair and its certificate from the root.
///
/// # Arguments
/// * `root` - the root's PKCS#8 DER private key.
/// * `developer` - the developer id.
/// * `expires` - Unix seconds.
///
/// # Returns
/// The developer key file (JSON).
///
/// # Errors
/// An invalid id, an unreadable root key, or key generation failed.
pub fn issue(root: &[u8], developer: &str, expires: u64) -> Result<String, String> {
    if !valid_developer(developer) {
        return Err(format!("'{developer}' is not a developer id ([a-z0-9][a-z0-9.-]{{0,63}})"));
    }
    let root = SigningKey::from_pkcs8_der(root).map_err(|e| format!("the root key cannot be read: {e}"))?;
    let signer = Signer::generate().map_err(|e| e.to_string())?;
    let certificate = root.sign(&message(developer, expires, &signer.public_key()));
    let key = signer.to_pkcs8_der().map_err(|e| e.to_string())?;
    let file = json!({ "developer": developer, "expires": expires, "key": to_hex(&key), "certificate": to_hex(&certificate.to_bytes()) });
    Ok(serde_json::to_string_pretty(&file).expect("JSON") + "\n")
}

impl DeveloperKey {
    /// Reads a developer key file and checks it was issued by `root` and has not
    /// expired.
    ///
    /// # Arguments
    /// * `file` - the key file's bytes.
    /// * `root` - the root public key.
    /// * `now` - Unix seconds.
    ///
    /// # Returns
    /// The verified key.
    ///
    /// # Errors
    /// Malformed JSON or fields, a certificate that does not verify, or an expired key.
    pub fn open(file: &[u8], root: &[u8; 32], now: u64) -> Result<DeveloperKey, String> {
        let value: Value = serde_json::from_slice(file).map_err(|e| format!("not a developer key file: {e}"))?;
        let field = |name: &str| value.get(name).ok_or_else(|| format!("the developer key file has no '{name}'"));
        let developer = field("developer")?.as_str().filter(|d| valid_developer(d)).ok_or("'developer' is not a developer id")?.to_owned();
        let expires = field("expires")?.as_u64().ok_or("'expires' is not Unix seconds")?;
        let hex = |name: &str| -> Result<Vec<u8>, String> { from_hex(field(name)?.as_str().ok_or(format!("'{name}' is not hex"))?) };
        let signer = Signer::from_pkcs8_der(&hex("key")?).map_err(|e| format!("'key': {e}"))?;
        let certificate: [u8; 64] = hex("certificate")?.try_into().map_err(|_| String::from("'certificate' is not 64 bytes"))?;
        verify(root, &developer, expires, &signer.public_key(), &certificate)?;
        if now > expires {
            return Err(format!("the key of '{developer}' expired"));
        }
        Ok(DeveloperKey { developer, expires, signer, certificate })
    }

    /// # Returns
    /// The [`METADATA`] entries a package signed with this key carries.
    pub fn metadata(&self) -> [(&'static str, String); 4] {
        [
            (METADATA[0], self.developer.clone()),
            (METADATA[1], self.expires.to_string()),
            (METADATA[2], to_hex(&self.signer.public_key())),
            (METADATA[3], to_hex(&self.certificate)),
        ]
    }
}

/// Checks that a package was signed with a developer key issued by `root`.
///
/// Decision: expiry is not checked here. It bounds when a key can sign; a
/// package signed before then stays valid.
///
/// # Arguments
/// * `package` - an opened (signature-verified) package.
/// * `root` - the root public key.
///
/// # Returns
/// The developer id.
///
/// # Errors
/// Missing certificate metadata, a package signer other than the certified key,
/// or a certificate that does not verify.
pub fn signed_by(package: &Package, root: &[u8; 32]) -> Result<String, String> {
    let metadata = &package.manifest().metadata;
    let get = |key: &str| metadata.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).ok_or(format!("the package has no '{key}'"));
    let developer = get(METADATA[0])?;
    let expires = get(METADATA[1])?.parse().map_err(|_| String::from("'developer.expires' is not a number"))?;
    let public: [u8; 32] = from_hex(get(METADATA[2])?)?.try_into().map_err(|_| String::from("'developer.key' is not 32 bytes"))?;
    let certificate: [u8; 64] = from_hex(get(METADATA[3])?)?.try_into().map_err(|_| String::from("'developer.certificate' is not 64 bytes"))?;
    if package.signer() != public {
        return Err(String::from("the package was not signed with the certified developer key"));
    }
    verify(root, developer, expires, &public, &certificate)?;
    Ok(developer.to_owned())
}
