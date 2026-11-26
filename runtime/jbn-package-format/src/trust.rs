//! Signing keys and the trust store that decides which signers may publish which
//! packages.
//!
//! Decision: a package carries one Ed25519 signature over a domain-separated
//! message holding the manifest's SHA-256, plus the signer's public key, as CBOR
//! `[public key, signature]`. jetbunny used detached COSE structures; nothing here
//! consumes COSE, so the plain form is used.
//!
//! The trust file is JSON:
//! `{"signers":[{"public_key":"<64 hex>","packages":["name", ...],"minimum_release":1}]}`;
//! an empty `packages` list trusts the key for every package.

use crate::Error;
use ed25519_dalek::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use ed25519_dalek::{Signature, Signer as _, SigningKey, Verifier, VerifyingKey};
use zeroize::Zeroizing;

/// Prefix of every signed message, so a package signature can never be taken
/// for a signature of anything else.
const DOMAIN: &[u8] = b"lunarstorm-jbn-v1\0";

/// # Returns
/// The message a signature covers.
fn message(digest: &[u8; 32]) -> Vec<u8> {
    [DOMAIN, digest].concat()
}

/// A private signing key.
pub struct Signer(SigningKey);

impl Signer {
    /// A new random key from the operating system's generator.
    ///
    /// # Errors
    /// The generator is unavailable.
    pub fn generate() -> Result<Signer, Error> {
        let mut seed = Zeroizing::new([0u8; 32]);
        getrandom::fill(seed.as_mut()).map_err(|e| Error::Io(e.to_string()))?;
        Ok(Signer(SigningKey::from_bytes(&seed)))
    }

    /// Reads a PKCS#8 DER private key (`openssl genpkey -algorithm ED25519 -outform DER`).
    ///
    /// # Errors
    /// Not an Ed25519 PKCS#8 key.
    pub fn from_pkcs8_der(bytes: &[u8]) -> Result<Signer, Error> {
        SigningKey::from_pkcs8_der(bytes).map(Signer).map_err(|e| Error::Trust(format!("signing key: {e}")))
    }

    /// # Returns
    /// The key as PKCS#8 DER; wiped from memory when dropped.
    ///
    /// # Errors
    /// Encoding failed.
    pub fn to_pkcs8_der(&self) -> Result<Zeroizing<Vec<u8>>, Error> {
        self.0
            .to_pkcs8_der()
            .map(|der| Zeroizing::new(der.as_bytes().to_vec()))
            .map_err(|e| Error::Trust(format!("signing key: {e}")))
    }

    /// # Returns
    /// The public key, as a trust store names it.
    pub fn public_key(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes()
    }

    /// # Returns
    /// `[public key, signature]` over the manifest digest, encoded.
    pub(crate) fn sign(&self, digest: &[u8; 32]) -> Vec<u8> {
        let signature = self.0.sign(&message(digest));
        crate::wire::encode(&crate::wire::Value::Array(vec![
            crate::wire::Value::Bytes(self.public_key().to_vec()),
            crate::wire::Value::Bytes(signature.to_bytes().to_vec()),
        ]))
    }
}

/// Reads the signature section.
///
/// # Returns
/// The claimed public key and the signature.
///
/// # Errors
/// Not `[32 bytes, 64 bytes]`.
pub(crate) fn signature(section: &[u8]) -> Result<([u8; 32], [u8; 64]), Error> {
    let value = crate::wire::decode(section)?;
    match value.array()? {
        [key, signature] => Ok((
            key.bytes()?.try_into().map_err(|_| Error::Format(String::from("public key is not 32 bytes")))?,
            signature.bytes()?.try_into().map_err(|_| Error::Format(String::from("signature is not 64 bytes")))?,
        )),
        _ => Err(Error::Format(String::from("signature must be [public key, signature]"))),
    }
}

/// One trusted signer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trusted {
    /// Its public key.
    pub public_key: [u8; 32],
    /// Package names it may sign; empty means any.
    pub packages: Vec<String>,
    /// The lowest release sequence accepted from it.
    pub minimum_release: u64,
}

/// The signers a host accepts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrustStore {
    /// Trusted signers.
    pub signers: Vec<Trusted>,
}

impl TrustStore {
    /// Parses a trust file (see the module documentation).
    ///
    /// # Errors
    /// Malformed JSON or keys.
    pub fn from_json(bytes: &[u8]) -> Result<TrustStore, Error> {
        let bad = |message: &str| Error::Trust(format!("trust file: {message}"));
        let json: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| bad(&e.to_string()))?;
        let list = json.get("signers").and_then(|s| s.as_array()).ok_or_else(|| bad("needs a signers array"))?;
        let signers = list
            .iter()
            .map(|entry| {
                let key = entry.get("public_key").and_then(|k| k.as_str()).ok_or_else(|| bad("public_key is required"))?;
                let public_key = from_hex(key).ok_or_else(|| bad("public_key must be 64 hex digits"))?;
                let packages = match entry.get("packages") {
                    None => Vec::new(),
                    Some(list) => list
                        .as_array()
                        .ok_or_else(|| bad("packages must be an array"))?
                        .iter()
                        .map(|p| p.as_str().map(str::to_owned).ok_or_else(|| bad("packages must be names")))
                        .collect::<Result<_, _>>()?,
                };
                let minimum_release = entry.get("minimum_release").map_or(Some(0), |m| m.as_u64()).ok_or_else(|| bad("minimum_release must be a number"))?;
                Ok(Trusted { public_key, packages, minimum_release })
            })
            .collect::<Result<_, Error>>()?;
        Ok(TrustStore { signers })
    }

    /// # Returns
    /// The trust file of this store.
    pub fn to_json(&self) -> String {
        let signers: Vec<_> = self
            .signers
            .iter()
            .map(|s| {
                serde_json::json!({
                    "public_key": to_hex(&s.public_key),
                    "packages": s.packages,
                    "minimum_release": s.minimum_release,
                })
            })
            .collect();
        serde_json::to_string_pretty(&serde_json::json!({ "signers": signers })).expect("json")
    }

    /// Checks a signature and the signer's authority over the package.
    ///
    /// # Errors
    /// [`Error::Trust`] when the key is not trusted for this package and release,
    /// or the signature does not verify.
    pub(crate) fn check(
        &self,
        public_key: &[u8; 32],
        signature: &[u8; 64],
        digest: &[u8; 32],
        package: &str,
        release: u64,
    ) -> Result<(), Error> {
        let trusted = self
            .signers
            .iter()
            .find(|s| &s.public_key == public_key && (s.packages.is_empty() || s.packages.iter().any(|p| p == package)))
            .ok_or_else(|| Error::Trust(format!("signer {} is not trusted for {package}", to_hex(public_key))))?;
        if release < trusted.minimum_release {
            return Err(Error::Trust(format!("release {release} is below the trusted minimum {}", trusted.minimum_release)));
        }
        let key = VerifyingKey::from_bytes(public_key).map_err(|e| Error::Trust(format!("public key: {e}")))?;
        key.verify(&message(digest), &Signature::from_bytes(signature))
            .map_err(|_| Error::Trust(String::from("the signature does not verify")))
    }
}

/// # Returns
/// Lowercase hex.
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// # Returns
/// 32 bytes from 64 hex digits.
fn from_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}
