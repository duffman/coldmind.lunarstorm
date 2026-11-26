//! Rust-owned native function graphs, independently verified before native loading.
#[derive(Clone, Debug, PartialEq)]
pub enum Constant {
    Number(f64),
    Bool(bool),
    Undefined,
    Null,
    String(Vec<u16>),
    /// Minimal little-endian signed two's complement; empty bytes encode zero.
    BigInt(Vec<u8>),
    Function(Box<FunctionImage>),
    /// Frozen tagged-template site object (BC_TAG_TEMPLATE_OBJECT): cooked
    /// strings, None for an invalid escape, and the frozen raw strings.
    TemplateObject {
        cooked: Vec<Option<Vec<u16>>>,
        raw: Vec<Vec<u16>>,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionFlags {
    pub has_prototype: bool,
    pub simple_parameters: bool,
    pub derived_constructor: bool,
    pub need_home_object: bool,
    pub kind: u8,
    pub new_target: bool,
    pub super_call: bool,
    pub super_allowed: bool,
    pub arguments_allowed: bool,
    pub backtrace_barrier: bool,
}
impl Default for FunctionFlags {
    fn default() -> Self {
        Self {
            has_prototype: false,
            simple_parameters: true,
            derived_constructor: false,
            need_home_object: false,
            kind: 0,
            new_target: false,
            super_call: false,
            super_allowed: false,
            arguments_allowed: false,
            backtrace_barrier: false,
        }
    }
}
/// Native lexical links address local slots, with distinct function/parameter tails.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScopeLink {
    #[default]
    End,
    Parameters,
    Local(u32),
}
impl ScopeLink {
    pub fn from_signed(n: i32) -> Result<Self, String> {
        match n {
            -1 => Ok(Self::End),
            -2 => Ok(Self::Parameters),
            0..=i32::MAX => Ok(Self::Local(n as u32)),
            _ => Err("invalid signed scope link".into()),
        }
    }
    pub fn to_signed(self) -> Result<i32, String> {
        match self {
            Self::End => Ok(-1),
            Self::Parameters => Ok(-2),
            Self::Local(n) => {
                i32::try_from(n).map_err(|_| "scope link exceeds native signed range".into())
            }
        }
    }
    pub fn local_index(self) -> Option<u32> {
        if let Self::Local(n) = self {
            Some(n)
        } else {
            None
        }
    }
    pub fn is_end(self) -> bool {
        matches!(self, Self::End)
    }
}
impl From<Option<u32>> for ScopeLink {
    fn from(n: Option<u32>) -> Self {
        n.map_or(Self::End, Self::Local)
    }
}
/// Materialized arguments local, proved against its special_object initializer.
/// Reuses the native four-bit variable kind; body declarations keep kind zero.
pub const VAR_KIND_ARGUMENTS: u8 = 12;
/// Anonymous active body-scope proof for a split parameter environment.
pub const VAR_KIND_BODY_ANCHOR: u8 = 13;
/// Body-scope proof whose distinct body arguments cell shadows the parameter cell.
pub const VAR_KIND_BODY_ARGUMENTS_ANCHOR: u8 = 14;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VarDef {
    pub name: Vec<u16>,
    pub scope_level: u32,
    pub scope_next: ScopeLink,
    pub is_const: bool,
    pub is_lexical: bool,
    pub is_captured: bool,
    pub var_ref_idx: u32,
    pub var_kind: u8,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Capture {
    pub name: Vec<u16>,
    pub var_idx: u32,
    pub closure_type: u8,
    pub is_const: bool,
    pub is_lexical: bool,
    pub var_kind: u8,
}

/// Rust compiler/artifact convention, distinct from unqualified native-only payloads.
pub const COMPILER_PROFILE: &str = "rust-native-locations-v3";
/// lunarstorm patch: `bc94` (was `bc93`), the native format version it names.
pub const NATIVE_PROFILE: &str = "native-bc94-unknown-coordinates";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DebugLocation {
    pub code_offset: u32,
    pub line: u32,
    pub column: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FunctionImage {
    pub code: Vec<u8>,
    pub constants: Vec<Constant>,
    pub local_count: u32,
    pub arg_count: u32,
    pub defined_arg_count: u32,
    pub stack_size: u32,
    pub strict: bool,
    pub function_name: String,
    pub filename: String,
    /// Actual 1-based native line/column locations, not compiler source spans.
    pub debug_locations: Vec<DebugLocation>,
    pub flags: FunctionFlags,
    /// Native vardefs: argument slots first, followed by local slots.
    pub locals: Vec<VarDef>,
    pub captures: Vec<Capture>,
    pub var_ref_count: u32,
    /// Symbolic UTF16 atoms; instruction atom operands are zero-based indices
    /// into this table. Encoding relocates them into the native atom namespace.
    pub atoms: Vec<Vec<u16>>,
    /// Original function source code units, encoded as native raw WTF8.
    pub source: Option<Vec<u16>>,
    /// lunarstorm patch: handler targets (code offsets) of user `catch`
    /// clauses, strictly ascending. Every other `catch` target is a
    /// finally/cleanup handler. `None` when the compiler recorded no table
    /// (debug markers off); `Some(vec![])` is a table with no user catch.
    ///
    /// Decision: debugger metadata in the debug section instead of jetbunny's
    /// `debug_catch` opcode, which used the last free one-byte opcode id while
    /// executing exactly like `catch`. First field of the native debug
    /// section: `count + 1` (0 = `None`), then each entry as a LEB delta.
    pub user_catches: Option<Vec<u32>>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum ModuleExport {
    Local {
        capture: u32,
        name: Vec<u16>,
    },
    Indirect {
        module: u32,
        import: Vec<u16>,
        name: Vec<u16>,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleImport {
    pub capture: u32,
    pub name: Vec<u16>,
    pub module: u32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleImage {
    pub name: Vec<u16>,
    pub required_modules: Vec<Vec<u16>>,
    /// lunarstorm patch: import attributes of each required module, parallel
    /// to `required_modules` (key/value pairs in source order), or empty when
    /// no request has any. Native format: after each request's atom, a pair
    /// count and that many key/value atoms. Specifiers stay unique.
    pub required_attributes: Vec<Vec<(Vec<u16>, Vec<u16>)>>,
    pub exports: Vec<ModuleExport>,
    pub star_exports: Vec<u32>,
    pub imports: Vec<ModuleImport>,
    pub has_top_level_await: bool,
    pub function: FunctionImage,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Image {
    Function(FunctionImage),
    Module(ModuleImage),
}
#[derive(Clone, Debug)]
pub struct VerifiedImage {
    bytes: Vec<u8>,
    image: Image,
    digest: [u8; 32],
    spans: Option<SpanMetadata>,
    profile: &'static str,
}
impl VerifiedImage {
    pub fn spans(&self) -> Option<&SpanMetadata> {
        self.spans.as_ref()
    }
    pub fn profile(&self) -> &'static str {
        self.profile
    }
    pub fn serialize_artifact(&self) -> Result<Vec<u8>, ImageError> {
        artifact::serialize(self)
    }
    pub fn serialize_artifact_with_limits(&self, l: &ImageLimits) -> Result<Vec<u8>, ImageError> {
        artifact::serialize_with_limits(self, l)
    }

    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
    pub fn target_fingerprint(&self) -> &'static str {
        crate::production::TARGET_FINGERPRINT
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn module(&self) -> Option<&ModuleImage> {
        if let Image::Module(m) = &self.image {
            Some(m)
        } else {
            None
        }
    }
    pub fn function(&self) -> &FunctionImage {
        match &self.image {
            Image::Function(f) => f,
            Image::Module(m) => &m.function,
        }
    }
}
pub type ImageError = String;
fn csum(p: &[u8]) -> u32 {
    let mut h = 0u32;
    let mut i = 0;
    while i + 4 < p.len() {
        h = h
            .wrapping_add(u32::from_le_bytes(p[i..i + 4].try_into().unwrap()))
            .wrapping_mul(0x9e370001);
        i += 4;
    }
    let mut v = 0;
    // Pinned upstream intentionally has no switch case for a four-byte tail.
    // Preserve its format behavior rather than silently changing the checksum.
    if p.len() - i < 4 {
        for (j, b) in p[i..].iter().enumerate() {
            v |= u32::from(*b) << (8 * j);
        }
    }
    h.wrapping_add(v).wrapping_mul(0x9e370001)
}
#[path = "image_format.rs"]
mod format;
#[path = "image_verify.rs"]
mod graph_verify;
/// Independently verify a complete image graph and encode native bytes without a runtime.
pub fn encode(f: &FunctionImage) -> Result<Vec<u8>, ImageError> {
    encode_with_limits(f, &ImageLimits::default())
}
pub fn encode_module(m: &ModuleImage) -> Result<Vec<u8>, ImageError> {
    encode_module_with_limits(m, &ImageLimits::default())
}
pub fn encode_module_with_limits(m: &ModuleImage, l: &ImageLimits) -> Result<Vec<u8>, ImageError> {
    graph_verify::verify_module_with_limits(m, l)?;
    format::encode_module_with_limits(m, l)
}
pub fn decode_and_verify(bytes: &[u8]) -> Result<VerifiedImage, ImageError> {
    decode_and_verify_with_limits(bytes, &ImageLimits::default())
}
pub fn decode_and_verify_with_limits(
    bytes: &[u8],
    l: &ImageLimits,
) -> Result<VerifiedImage, ImageError> {
    Ok(decode_limited(bytes, l)?.0)
}
fn decode_limited(bytes: &[u8], l: &ImageLimits) -> Result<(VerifiedImage, usize), ImageError> {
    let (image, decode_work) = format::decode_with_limits(bytes, l)?;
    let remaining = ImageLimits {
        max_work: l.max_work - decode_work,
        ..*l
    };
    let semantic_work = match &image {
        Image::Function(f) => graph_verify::verify_with_limits(f, &remaining)?,
        Image::Module(m) => graph_verify::verify_module_with_limits(m, &remaining)?,
    };
    use sha2::{Digest, Sha256};
    Ok((
        VerifiedImage {
            bytes: bytes.to_vec(),
            image,
            digest: Sha256::digest(bytes).into(),
            spans: None,
            profile: NATIVE_PROFILE,
        },
        decode_work + semantic_work,
    ))
}
pub fn verify_and_encode(
    f: &FunctionImage,
    spans: Option<SpanMetadata>,
) -> Result<VerifiedImage, ImageError> {
    verify_and_encode_with_limits(f, spans, &ImageLimits::default())
}
pub fn verify_and_encode_with_limits(
    f: &FunctionImage,
    spans: Option<SpanMetadata>,
    l: &ImageLimits,
) -> Result<VerifiedImage, ImageError> {
    let graph_limits = artifact::graph_limits(spans.as_ref(), l)?;
    let used = graph_verify::verify_with_limits(f, &graph_limits)?;
    let bytes = format::encode_with_limits(f, &graph_limits)?;
    let remaining = ImageLimits {
        max_work: l.max_work - used,
        ..graph_limits
    };
    let (mut image, decoded_work) = decode_limited(&bytes, &remaining)?;
    if let Some(meta) = &spans {
        let remaining = ImageLimits {
            max_work: remaining.max_work - decoded_work,
            ..*l
        };
        artifact::verify_spans_with_limits(image.function(), meta, &remaining)?;
    }

    image.spans = spans;
    image.profile = COMPILER_PROFILE;
    Ok(image)
}
pub fn verify_and_encode_module(
    m: &ModuleImage,
    spans: Option<SpanMetadata>,
) -> Result<VerifiedImage, ImageError> {
    verify_and_encode_module_with_limits(m, spans, &ImageLimits::default())
}
pub fn verify_and_encode_module_with_limits(
    m: &ModuleImage,
    spans: Option<SpanMetadata>,
    l: &ImageLimits,
) -> Result<VerifiedImage, ImageError> {
    let graph_limits = artifact::graph_limits(spans.as_ref(), l)?;
    let used = graph_verify::verify_module_with_limits(m, &graph_limits)?;
    let bytes = format::encode_module_with_limits(m, &graph_limits)?;
    let remaining = ImageLimits {
        max_work: l.max_work - used,
        ..graph_limits
    };
    let (mut image, decoded_work) = decode_limited(&bytes, &remaining)?;
    if let Some(meta) = &spans {
        let remaining = ImageLimits {
            max_work: remaining.max_work - decoded_work,
            ..*l
        };
        artifact::verify_spans_with_limits(image.function(), meta, &remaining)?;
    }

    image.spans = spans;
    image.profile = COMPILER_PROFILE;
    Ok(image)
}
#[path = "image_artifact.rs"]
mod artifact;
pub use artifact::{
    deserialize_artifact, deserialize_artifact_with_limits, source_digest_utf16, FunctionSpans,
    SourceUnit, SpanMetadata,
};
#[cfg(test)]
mod checksum_tests {
    #[test]
    fn pinned_checksum_ignores_exact_four_byte_tail() {
        assert_eq!(super::csum(&[1, 2, 3, 4]), 0);
        assert_eq!(super::csum(&[1, 2, 3, 4, 5, 6, 7, 8]), 477168129);
        assert_eq!(
            super::csum(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]),
            4074047494
        );
    }
}

#[cfg(test)]
mod module_decode_tests {
    use super::*;
    use crate::production::ids;
    #[test]
    fn malformed_native_module_records_never_obtain_a_proof() {
        let mut function = FunctionImage {
            code: vec![
                ids::push_this,
                ids::if_false,
                5,
                0,
                0,
                0,
                ids::return_undef,
                ids::undefined,
                ids::return_async,
            ],
            stack_size: 1,
            strict: true,
            filename: "test.mjs".into(),
            ..Default::default()
        };
        function.flags.kind = 2;
        let mut m = ModuleImage {
            name: vec![b'm' as u16],
            required_modules: vec![],
            required_attributes: vec![],
            exports: vec![],
            star_exports: vec![],
            imports: vec![],
            has_top_level_await: false,
            function,
        };
        let valid = format::encode_module(&m).unwrap();
        decode_and_verify(&valid).unwrap();
        for end in 0..valid.len() {
            assert!(decode_and_verify(&valid[..end]).is_err())
        }
        m.exports.push(ModuleExport::Local {
            capture: 65535,
            name: vec![b'x' as u16],
        });
        let invalid = format::encode_module(&m).unwrap();
        assert!(
            decode_and_verify(&invalid).is_err(),
            "valid checksum does not prove module indices"
        );
        m.exports.clear();
        // Module roots are async, so top-level await is an accepted flag.
        m.has_top_level_await = true;
        decode_and_verify(&format::encode_module(&m).unwrap()).unwrap();
        m.has_top_level_await = false;
        m.function.code[6] = ids::nop;
        assert!(
            decode_and_verify(&format::encode_module(&m).unwrap()).is_err(),
            "native async completion cannot run in initialization frame"
        );
    }
}

#[path = "image_eval.rs"]
pub mod eval;

#[path = "image_limits.rs"]
mod limits;
pub use limits::ImageLimits;
pub fn encode_with_limits(f: &FunctionImage, limits: &ImageLimits) -> Result<Vec<u8>, ImageError> {
    graph_verify::verify_with_limits(f, limits)?;
    format::encode_with_limits(f, limits)
}
