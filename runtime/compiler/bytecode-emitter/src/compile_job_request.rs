//! Versioned owned compiler jobs. No native handles or frame authority cross this format.
use crate::{EmissionLimits, job_protocol as legacy};
use qjs_bytecode_image_verifier::image::{
    self, FunctionFlags, ScopeLink,
    eval::{EnvBinding, EnvLayout, VerifiedEnvLayout},
};
use qjs_js_parser_frontend::CompileLimits;

pub const MAX_ENV_BYTES: usize = legacy::MAX_ENV_BYTES;
pub const MAX_REQUEST_BYTES: usize = legacy::MAX_REQUEST_BYTES;
const MAGIC: &[u8; 8] = b"QJSJOB02";
const RESPONSE: &[u8; 8] = b"QJSRES02";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Script,
    Module,
    Eval,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Budgets {
    pub frontend: CompileLimits,
    pub emission: EmissionLimits,
    pub image: image::ImageLimits,
}
impl Budgets {
    pub fn validate(&self) -> Result<(), String> {
        let defaults = Self::default();
        if self
            .values()
            .iter()
            .zip(defaults.values())
            .any(|(a, b)| *a > b)
        {
            return Err("capacity: limit override".into());
        }
        Ok(())
    }
    fn values(self) -> [usize; 26] {
        let f = self.frontend;
        let e = self.emission;
        let i = self.image;
        [
            f.max_ast_nodes,
            f.max_nesting,
            f.max_functions,
            f.max_bindings,
            e.max_ir_nodes,
            e.max_nesting,
            e.max_functions,
            e.max_locals,
            e.max_atoms,
            e.max_atom_units,
            e.max_constants,
            e.max_instructions,
            e.max_code_bytes,
            e.max_positions,
            e.max_source_units,
            e.max_work,
            i.max_native_bytes,
            i.max_graph_bytes,
            i.max_functions,
            i.max_depth,
            i.max_table_entries,
            i.max_instructions,
            i.max_positions,
            i.max_state_cells,
            i.max_work,
            i.max_artifact_bytes,
        ]
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, String> {
        let mut v = [0; 26];
        for n in &mut v {
            *n = usize::try_from(r.u64()?).map_err(|_| "capacity: limit width")?;
        }
        let result = Self {
            frontend: CompileLimits {
                max_ast_nodes: v[0],
                max_nesting: v[1],
                max_functions: v[2],
                max_bindings: v[3],
            },
            emission: EmissionLimits {
                max_ir_nodes: v[4],
                max_nesting: v[5],
                max_functions: v[6],
                max_locals: v[7],
                max_atoms: v[8],
                max_atom_units: v[9],
                max_constants: v[10],
                max_instructions: v[11],
                max_code_bytes: v[12],
                max_positions: v[13],
                max_source_units: v[14],
                max_work: v[15],
            },
            image: image::ImageLimits {
                max_native_bytes: v[16],
                max_graph_bytes: v[17],
                max_functions: v[18],
                max_depth: v[19],
                max_table_entries: v[20],
                max_instructions: v[21],
                max_positions: v[22],
                max_state_cells: v[23],
                max_work: v[24],
                max_artifact_bytes: v[25],
            },
        };
        result.validate()?;
        Ok(result)
    }
}
pub struct Request {
    pub mode: Mode,
    pub source: Vec<u16>,
    pub filename: String,
    pub budgets: Budgets,
    pub environment: Option<VerifiedEnvLayout>,
    pub digest: [u8; 32],
}
fn num(b: &mut Vec<u8>, v: u32) {
    b.extend(v.to_le_bytes())
}
fn blob(b: &mut Vec<u8>, v: &[u8]) {
    num(b, v.len() as u32);
    b.extend(v)
}
fn units(b: &mut Vec<u8>, v: &[u16]) {
    num(b, v.len() as u32);
    for u in v {
        b.extend(u.to_le_bytes());
    }
}
fn link(b: &mut Vec<u8>, v: ScopeLink) -> Result<(), String> {
    b.extend(v.to_signed()?.to_le_bytes());
    Ok(())
}
fn flags(b: &mut Vec<u8>, f: &FunctionFlags) {
    b.push(f.kind);
    let bits = [
        f.has_prototype,
        f.simple_parameters,
        f.derived_constructor,
        f.need_home_object,
        f.new_target,
        f.super_call,
        f.super_allowed,
        f.arguments_allowed,
        f.backtrace_barrier,
    ]
    .iter()
    .enumerate()
    .fold(0u16, |n, (i, x)| n | (u16::from(*x) << i));
    b.extend(bits.to_le_bytes());
}
fn environment(v: &VerifiedEnvLayout) -> Result<Vec<u8>, String> {
    let mut b = Vec::new();
    let e = v.layout();
    b.push(u8::from(e.strict));
    flags(&mut b, &e.flags);
    num(&mut b, e.var_ref_count);
    link(&mut b, e.scope_head)?;
    for group in [&e.args, &e.locals, &e.refs] {
        if group.len() > 65535 || b.len().saturating_add(4) > MAX_ENV_BYTES {
            return Err("capacity: environment bindings".into());
        }
        num(&mut b, group.len() as u32);
        for x in group {
            let required = x
                .name
                .len()
                .checked_mul(2)
                .and_then(|n| n.checked_add(18))
                .ok_or("capacity: environment bytes")?;
            if b.len()
                .checked_add(required)
                .is_none_or(|n| n > MAX_ENV_BYTES)
            {
                return Err("capacity: environment bytes".into());
            }
            units(&mut b, &x.name);
            let bits = [
                x.is_const,
                x.is_lexical,
                x.is_captured,
                x.initialized,
                x.live,
            ]
            .iter()
            .enumerate()
            .fold(0u8, |n, (i, x)| n | (u8::from(*x) << i));
            b.push(bits);
            b.push(x.var_kind);
            num(&mut b, x.scope_level);
            link(&mut b, x.scope_next)?;
            num(&mut b, x.var_ref_idx);
            if b.len() > MAX_ENV_BYTES {
                return Err("capacity: environment bytes".into());
            }
        }
    }
    Ok(b)
}
/// Hash exact bytes, including their length and odd-byte padding. Uses the existing SHA256 implementation.
pub fn digest(bytes: &[u8]) -> [u8; 32] {
    let mut words = Vec::with_capacity(4 + bytes.len().div_ceil(2));
    for c in (bytes.len() as u64).to_le_bytes().as_chunks::<2>().0 {
        words.push(u16::from_le_bytes(*c));
    }
    for c in bytes.chunks(2) {
        words.push(u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)]));
    }
    image::source_digest_utf16(&words)
}
pub fn encode(
    mode: Mode,
    source: &[u16],
    filename: &str,
    budgets: Budgets,
    env: Option<&VerifiedEnvLayout>,
) -> Result<Vec<u8>, String> {
    budgets.validate()?;
    if source.len() > legacy::MAX_SOURCE_BYTES / 2 || filename.len() > legacy::MAX_FILENAME_BYTES {
        return Err("capacity: source or filename".into());
    }
    if filename.contains('\0') || matches!(mode, Mode::Eval) != env.is_some() {
        return Err("compiler mode/environment mismatch".into());
    }
    let mut b = MAGIC.to_vec();
    b.push(match mode {
        Mode::Script => 0,
        Mode::Module => 1,
        Mode::Eval => 2,
    });
    blob(&mut b, image::COMPILER_PROFILE.as_bytes());
    blob(
        &mut b,
        qjs_bytecode_image_verifier::production::TARGET_FINGERPRINT.as_bytes(),
    );
    for v in budgets.values() {
        b.extend((v as u64).to_le_bytes());
    }
    blob(&mut b, filename.as_bytes());
    units(&mut b, source);
    match env {
        None => b.push(0),
        Some(e) => {
            b.push(1);
            b.extend(e.digest());
            blob(&mut b, &environment(e)?);
        }
    }
    if b.len() > MAX_REQUEST_BYTES - 32 {
        return Err("capacity: request bytes".into());
    }
    let hash = digest(&b);
    b.extend(hash);
    Ok(b)
}
pub fn decode(bytes: &[u8]) -> Result<Request, String> {
    if bytes.len() < 41 || bytes.len() > MAX_REQUEST_BYTES {
        return Err("capacity: request bytes".into());
    }
    let end = bytes.len() - 32;
    let hash = digest(&bytes[..end]);
    if hash != bytes[end..] {
        return Err("request integrity mismatch".into());
    }
    let mut r = Reader {
        b: &bytes[..end],
        at: 0,
    };
    if r.take(8)? != MAGIC {
        return Err("compiler request version mismatch".into());
    }
    let mode = match r.byte()? {
        0 => Mode::Script,
        1 => Mode::Module,
        2 => Mode::Eval,
        _ => return Err("compiler request mode".into()),
    };
    if r.blob(128)? != image::COMPILER_PROFILE.as_bytes()
        || r.blob(128)? != qjs_bytecode_image_verifier::production::TARGET_FINGERPRINT.as_bytes()
    {
        return Err("compiler request profile/target mismatch".into());
    }
    let budgets = Budgets::read(&mut r)?;
    let filename = std::str::from_utf8(r.blob(legacy::MAX_FILENAME_BYTES)?)
        .map_err(|_| "invalid filename UTF8")?
        .to_owned();
    if filename.contains('\0') {
        return Err("filename NUL".into());
    }
    let source = r.units(legacy::MAX_SOURCE_BYTES / 2)?;
    let environment = match r.byte()? {
        0 => None,
        1 => {
            let expected = r.take(32)?.to_vec();
            let bytes = r.blob(MAX_ENV_BYTES)?;
            let mut e = Reader { b: bytes, at: 0 };
            let strict = e.boolean()?;
            let kind = e.byte()?;
            let bits = u16::from_le_bytes(e.take(2)?.try_into().unwrap());
            if bits > 511 {
                return Err("environment flags".into());
            }
            let flag = |i: u32| bits & (1u16 << i) != 0;
            let flags = FunctionFlags {
                kind,
                has_prototype: flag(0),
                simple_parameters: flag(1),
                derived_constructor: flag(2),
                need_home_object: flag(3),
                new_target: flag(4),
                super_call: flag(5),
                super_allowed: flag(6),
                arguments_allowed: flag(7),
                backtrace_barrier: flag(8),
            };
            let var_ref_count = e.u32()?;
            let scope_head = e.link()?;
            let mut groups = Vec::new();
            for _ in 0..3 {
                let count = e.u32()? as usize;
                if count > 65535 || count > e.remaining() / 18 {
                    return Err("capacity: environment bindings".into());
                }
                let mut bindings = Vec::new();
                for _ in 0..count {
                    let name = e.units(MAX_ENV_BYTES / 2)?;
                    let bits = e.byte()?;
                    if bits > 31 {
                        return Err("environment binding flags".into());
                    }
                    bindings.push(EnvBinding {
                        name,
                        is_const: bits & 1 != 0,
                        is_lexical: bits & 2 != 0,
                        is_captured: bits & 4 != 0,
                        initialized: bits & 8 != 0,
                        live: bits & 16 != 0,
                        var_kind: e.byte()?,
                        scope_level: e.u32()?,
                        scope_next: e.link()?,
                        var_ref_idx: e.u32()?,
                    });
                }
                groups.push(bindings);
            }
            if e.remaining() != 0 {
                return Err("trailing environment data".into());
            }
            let refs = groups.pop().unwrap();
            let locals = groups.pop().unwrap();
            let args = groups.pop().unwrap();
            let verified = image::eval::verify_env_layout(EnvLayout {
                strict,
                flags,
                var_ref_count,
                scope_head,
                args,
                locals,
                refs,
                ..Default::default()
            })?;
            if verified.digest() != expected.as_slice() {
                return Err("environment digest mismatch".into());
            }
            Some(verified)
        }
        _ => return Err("environment presence flag".into()),
    };
    if r.remaining() != 0 || matches!(mode, Mode::Eval) != environment.is_some() {
        return Err("request trailing data or mode/environment mismatch".into());
    }
    Ok(Request {
        mode,
        source,
        filename,
        budgets,
        environment,
        digest: hash,
    })
}
pub fn response(mode: Mode, request_digest: &[u8; 32], artifact: &[u8]) -> Result<Vec<u8>, String> {
    if artifact.len() > legacy::MAX_ARTIFACT_BYTES {
        return Err("capacity: artifact".into());
    }
    let mut b = RESPONSE.to_vec();
    b.push(match mode {
        Mode::Script => 0,
        Mode::Module => 1,
        Mode::Eval => 2,
    });
    b.extend(request_digest);
    blob(&mut b, artifact);
    Ok(b)
}
pub fn read_response<'a>(
    bytes: &'a [u8],
    mode: Mode,
    request_digest: &[u8; 32],
) -> Result<&'a [u8], String> {
    let mut r = Reader { b: bytes, at: 0 };
    if r.take(8)? != RESPONSE
        || r.byte()?
            != match mode {
                Mode::Script => 0,
                Mode::Module => 1,
                Mode::Eval => 2,
            }
        || r.take(32)? != request_digest
    {
        return Err("worker response request/mode mismatch".into());
    }
    let artifact = r.blob(legacy::MAX_ARTIFACT_BYTES)?;
    if r.remaining() != 0 {
        return Err("trailing worker response".into());
    }
    Ok(artifact)
}
struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.b.len() - self.at
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).ok_or("request overflow")?;
        let s = self
            .b
            .get(self.at..end)
            .ok_or("truncated compiler request")?;
        self.at = end;
        Ok(s)
    }
    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn boolean(&mut self) -> Result<bool, String> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err("invalid boolean".into()),
        }
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn link(&mut self) -> Result<ScopeLink, String> {
        ScopeLink::from_signed(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn blob(&mut self, max: usize) -> Result<&'a [u8], String> {
        let n = self.u32()? as usize;
        if n > max {
            return Err("capacity: request field".into());
        }
        self.take(n)
    }
    fn units(&mut self, max: usize) -> Result<Vec<u16>, String> {
        let n = self.u32()? as usize;
        if n > max {
            return Err("capacity: UTF16 field".into());
        }
        let raw = self.take(n.checked_mul(2).ok_or("UTF16 overflow")?)?;
        Ok(raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn v2_request_binds_exact_units_mode_target_limits_and_response() {
        let source = [39, 0xd800, 39, 59];
        let bytes = encode(Mode::Script, &source, "owned.js", Budgets::default(), None).unwrap();
        let request = decode(&bytes).unwrap();
        assert_eq!(request.source, source);
        assert_eq!(request.mode, Mode::Script);
        for at in [0, 8, 20, 128, bytes.len() - 1] {
            let mut bad = bytes.clone();
            bad[at] ^= 1;
            assert!(decode(&bad).is_err());
        }
        let response = response(Mode::Script, &request.digest, b"protected artifact").unwrap();
        assert!(read_response(&response, Mode::Module, &request.digest).is_err());
        assert!(read_response(&response, Mode::Script, &[0; 32]).is_err());
        assert_eq!(
            read_response(&response, Mode::Script, &request.digest).unwrap(),
            b"protected artifact"
        );
        let mut relaxed = Budgets::default();
        relaxed.frontend.max_ast_nodes += 1;
        assert!(encode(Mode::Script, &[], "x", relaxed, None).is_err());
        assert!(encode(Mode::Eval, &[], "x", Budgets::default(), None).is_err());
    }
    #[test]
    fn eval_environment_is_owned_and_verified_and_cannot_become_ordinary_source() {
        let env = image::eval::verify_env_layout(EnvLayout::default()).unwrap();
        let bytes = encode(Mode::Eval, &[49], "eval.js", Budgets::default(), Some(&env)).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.environment.unwrap().digest(), env.digest());
        assert!(encode(Mode::Script, &[], "x", Budgets::default(), Some(&env)).is_err());
        let mut bad = bytes;
        bad[8] = 0;
        let end = bad.len() - 32;
        let h = digest(&bad[..end]);
        bad[end..].copy_from_slice(&h);
        assert!(decode(&bad).is_err());
    }
}
