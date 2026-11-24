//! npm Subresource Integrity normalization and verification.
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha384, Sha512};

/// Prefer SHA-512, then SHA-384/SHA-256, and finally SHA-1.
/// npm's old `dist.shasum` is used only if `dist.integrity` is absent.
pub fn canonical(integrity: Option<&str>, shasum: Option<&str>) -> Result<String> {
    if let Some(value) = integrity {
        for algorithm in ["sha512", "sha384", "sha256", "sha1"] {
            if let Some(entry) = value
                .split_whitespace()
                .find(|part| part.starts_with(&format!("{algorithm}-")))
            {
                let encoded = entry.split_once('-').expect("prefix checked").1;
                let bytes = STANDARD
                    .decode(encoded)
                    .context("invalid base64 integrity")?;
                let expected_len = match algorithm {
                    "sha512" => 64,
                    "sha384" => 48,
                    "sha256" => 32,
                    _ => 20,
                };
                if bytes.len() != expected_len {
                    bail!("incorrect {algorithm} digest length: {}", bytes.len());
                }
                return Ok(entry.to_string());
            }
        }
        bail!("unsupported npm integrity algorithm: {value}");
    }
    let shasum = shasum.context("package has neither dist.integrity nor dist.shasum")?;
    let bytes = hex::decode(shasum).context("invalid dist.shasum")?;
    if bytes.len() != 20 {
        bail!("dist.shasum must be 20 bytes");
    }
    Ok(format!("sha1-{}", STANDARD.encode(bytes)))
}

/// Check tarball bytes against a normalized SRI hash before extraction or caching.
pub fn verify(data: &[u8], expected: &str) -> Result<()> {
    let (algorithm, encoded) = expected
        .split_once('-')
        .context("malformed npm integrity string")?;
    let digest = match algorithm {
        "sha512" => Sha512::digest(data).to_vec(),
        "sha384" => Sha384::digest(data).to_vec(),
        "sha256" => Sha256::digest(data).to_vec(),
        "sha1" => Sha1::digest(data).to_vec(),
        other => bail!("unsupported integrity algorithm: {other}"),
    };
    let expected_bytes = STANDARD
        .decode(encoded)
        .context("invalid base64 integrity")?;
    if digest != expected_bytes {
        bail!("npm package integrity verification failed ({algorithm})");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_sha512() {
        let data = b"verified package";
        let check = format!("sha512-{}", STANDARD.encode(Sha512::digest(data)));
        verify(data, &check).unwrap();
        assert!(verify(b"tampered", &check).is_err());
    }

    #[test]
    fn accepts_legacy_shasum() {
        let data = b"legacy";
        let check = canonical(None, Some(&hex::encode(Sha1::digest(data)))).unwrap();
        verify(data, &check).unwrap();
    }

    #[test]
    fn prefers_stronger_hash() {
        let check = format!(
            "sha1-{} sha512-{}",
            STANDARD.encode([0_u8; 20]),
            STANDARD.encode([0_u8; 64])
        );
        assert!(canonical(Some(&check), None)
            .unwrap()
            .starts_with("sha512-"));
    }
}
