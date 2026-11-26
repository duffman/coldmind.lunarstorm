//! Public default request codec and hard worker bounds.
//! Script/Module helpers emit the same version2 jobs as the bounded parent.
//! The superseded version1 implementation is archived, not a worker fallback.
pub const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_HEAP_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_FILENAME_BYTES: usize = 64 * 1024;
pub const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;
pub const MAX_ENV_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_REQUEST_BYTES: usize = MAX_SOURCE_BYTES + MAX_FILENAME_BYTES + MAX_ENV_BYTES + 1040;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Script,
    Module,
}
pub fn encode(source: &[u16], filename: &str) -> Result<Vec<u8>, String> {
    encode_request(Kind::Script, source, filename)
}
pub fn encode_request(kind: Kind, source: &[u16], filename: &str) -> Result<Vec<u8>, String> {
    crate::job_request::encode(
        match kind {
            Kind::Script => crate::job_request::Mode::Script,
            Kind::Module => crate::job_request::Mode::Module,
        },
        source,
        filename,
        Default::default(),
        None,
    )
}
pub fn decode(bytes: &[u8]) -> Result<(Vec<u16>, String), String> {
    let (kind, source, filename) = decode_request(bytes)?;
    if kind != Kind::Script {
        return Err("expected script compiler request".into());
    }
    Ok((source, filename))
}
pub fn decode_request(bytes: &[u8]) -> Result<(Kind, Vec<u16>, String), String> {
    let request = crate::job_request::decode(bytes)?;
    let kind = match request.mode {
        crate::job_request::Mode::Script => Kind::Script,
        crate::job_request::Mode::Module => Kind::Module,
        crate::job_request::Mode::Eval => {
            return Err("eval requires an owned environment-aware request".into());
        }
    };
    Ok((kind, request.source, request.filename))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requests_preserve_lone_surrogates_and_reject_malformed_lengths() {
        let source = [0xd800, 0, 0xdc00, 42];
        let bytes = encode(&source, "å.js").unwrap();
        assert_eq!(decode(&bytes).unwrap(), (source.to_vec(), "å.js".into()));
        for end in 0..bytes.len() {
            assert!(decode(&bytes[..end]).is_err());
        }
        let mut trailing = bytes;
        trailing.push(0);
        assert!(decode(&trailing).is_err());
        assert!(encode(&[], "nul\0name").is_err());
    }
    #[test]
    fn request_source_capacity_boundary() {
        for extra in [0, 1, 2] {
            let source = vec![32; MAX_SOURCE_BYTES / 2 - 1 + extra];
            assert_eq!(encode(&source, "boundary").is_ok(), extra < 2);
        }
        let mut forged = encode(&[], "").unwrap();
        forged[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&forged).is_err());
    }
}
