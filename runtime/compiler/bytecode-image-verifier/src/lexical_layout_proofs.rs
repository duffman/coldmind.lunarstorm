//! Pure lexical-layout proofs. Live-frame authority remains with the native owner.
use super::{
    Capture, FunctionFlags, FunctionImage, Image, ImageError, ScopeLink, VAR_KIND_ARGUMENTS,
    VAR_KIND_BODY_ANCHOR, VAR_KIND_BODY_ARGUMENTS_ANCHOR,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EnvBinding {
    pub name: Vec<u16>,
    pub is_const: bool,
    pub is_lexical: bool,
    pub var_kind: u8,
    pub scope_level: u32,
    pub scope_next: ScopeLink,
    pub is_captured: bool,
    pub var_ref_idx: u32,
    pub initialized: bool,
    /// Locals/arguments have a reserved live-reference slot; inherited refs are nonnull.
    pub live: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct EnvLayout {
    pub target_fingerprint: String,
    pub args: Vec<EnvBinding>,
    pub locals: Vec<EnvBinding>,
    pub refs: Vec<EnvBinding>,
    pub var_ref_count: u32,
    pub scope_head: ScopeLink,
    pub strict: bool,
    pub flags: FunctionFlags,
}
impl Default for EnvLayout {
    fn default() -> Self {
        Self {
            target_fingerprint: crate::production::TARGET_FINGERPRINT.into(),
            args: vec![],
            locals: vec![],
            refs: vec![],
            var_ref_count: 0,
            scope_head: ScopeLink::End,
            strict: false,
            flags: FunctionFlags::default(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Slot {
    Local(u32),
    Arg(u32),
    Ref(u32),
}
#[derive(Clone, Debug, PartialEq)]
pub struct VisibleBinding {
    pub slot: Slot,
    pub binding: EnvBinding,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HiddenBinding {
    This,
    Arguments,
    NewTarget,
    HomeObject,
    ActiveFunction,
    VarObject,
    ArgVarObject,
}
impl HiddenBinding {
    fn name(self) -> &'static str {
        match self {
            Self::This => "this",
            Self::Arguments => "arguments",
            Self::NewTarget => "new.target",
            Self::HomeObject => "<home_object>",
            Self::ActiveFunction => "this.active_func",
            Self::VarObject => "<var>",
            Self::ArgVarObject => "<arg_var>",
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedEnvLayout {
    layout: EnvLayout,
    digest: [u8; 32],
    bindings: Vec<VisibleBinding>,
    /// Every binding in resolution order, shadowed ones included.
    chain: Vec<VisibleBinding>,
    parameter_scope: bool,
    indices: BTreeMap<Vec<u16>, usize>,
}
impl VerifiedEnvLayout {
    pub fn layout(&self) -> &EnvLayout {
        &self.layout
    }
    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
    /// Visible bindings in resolution order, with shadowed names removed.
    pub fn bindings(&self) -> &[VisibleBinding] {
        &self.bindings
    }
    /// All bindings in resolution order, shadowed `<with>`/`<var>` objects included.
    pub fn chain(&self) -> &[VisibleBinding] {
        &self.chain
    }
    pub fn parameter_scope(&self) -> bool {
        self.parameter_scope
    }
    pub fn binding(&self, name: &[u16]) -> Option<&VisibleBinding> {
        self.indices.get(name).map(|at| &self.bindings[*at])
    }
    pub fn hidden(&self, role: HiddenBinding) -> Option<&VisibleBinding> {
        self.binding(&role.name().encode_utf16().collect::<Vec<_>>())
    }
    pub(super) fn readonly_hidden(&self, name: &[u16]) -> bool {
        [
            HiddenBinding::This,
            HiddenBinding::NewTarget,
            HiddenBinding::HomeObject,
            HiddenBinding::ActiveFunction,
            HiddenBinding::VarObject,
            HiddenBinding::ArgVarObject,
        ]
        .iter()
        .any(|role| name == role.name().encode_utf16().collect::<Vec<_>>())
    }
    pub(super) fn check_capture(&self, c: &Capture) -> Result<(), String> {
        let at = |v: &VisibleBinding| match v.slot {
            Slot::Local(n) => (0, n),
            Slot::Arg(n) => (1, n),
            Slot::Ref(n) => (2, n),
        };
        // Shadowed `<with>`/`<var>` scope objects stay reachable: eval tests each in turn.
        let v = if is_name_units(&c.name, "<with>") || is_name_units(&c.name, "<var>") {
            self.chain
                .iter()
                .find(|v| v.binding.name == c.name && at(v) == (c.closure_type, c.var_idx))
        } else {
            self.binding(&c.name)
        }
        .ok_or("eval capture is not visible")?;
        let expected = at(v);
        if (c.closure_type, c.var_idx) != expected
            || c.is_const != v.binding.is_const
            || c.is_lexical != v.binding.is_lexical
            || c.var_kind != v.binding.var_kind
        {
            return Err("eval capture layout mismatch".into());
        }
        if !v.binding.initialized && !v.binding.is_lexical {
            return Err("unmodeled nonlexical uninitialized binding".into());
        }
        if !v.binding.live
            || matches!(v.slot, Slot::Local(_) | Slot::Arg(_)) && !v.binding.is_captured
        {
            return Err("eval binding has no live reference slot".into());
        }
        Ok(())
    }
}
fn is_arg_pseudo(v: &EnvBinding) -> bool {
    matches!(v.var_kind, 4 | VAR_KIND_ARGUMENTS)
        || [
            HiddenBinding::This,
            HiddenBinding::NewTarget,
            HiddenBinding::HomeObject,
            HiddenBinding::ActiveFunction,
            HiddenBinding::ArgVarObject,
        ]
        .iter()
        .any(|r| v.name == r.name().encode_utf16().collect::<Vec<_>>())
}
fn is_name_units(name: &[u16], s: &str) -> bool {
    name == s.encode_utf16().collect::<Vec<_>>()
}
fn is_name(v: &EnvBinding, s: &str) -> bool {
    v.name == s.encode_utf16().collect::<Vec<_>>()
}
/// Verify only owned metadata. This proof does not authorize access to a live frame.
pub fn verify_env_layout(layout: EnvLayout) -> Result<VerifiedEnvLayout, ImageError> {
    if layout.target_fingerprint != crate::production::TARGET_FINGERPRINT {
        return Err("eval environment target mismatch".into());
    }
    if layout.args.len() > 65535
        || layout.locals.len() > 65535
        || layout.refs.len() > 65535
        || layout.var_ref_count > 65535
    {
        return Err("capacity: eval environment table budget".into());
    }
    if layout.flags.kind > 3 {
        return Err("caller function kind".into());
    }
    if layout
        .scope_head
        .local_index()
        .is_some_and(|n| n as usize >= layout.locals.len())
    {
        return Err("eval scope head".into());
    }
    let mut budget = 16 * 1024 * 1024usize;
    let mut refs = BTreeSet::new();
    for v in layout.args.iter().chain(&layout.locals).chain(&layout.refs) {
        budget = budget
            .checked_sub(std::mem::size_of::<EnvBinding>() + v.name.len().saturating_mul(2))
            .ok_or("capacity: eval layout allocation budget")?;
        // 0..=4 ordinary kinds, 5..=9 QuickJS private field/method/accessors.
        if (v.var_kind > 9
            && !matches!(
                v.var_kind,
                VAR_KIND_ARGUMENTS | VAR_KIND_BODY_ANCHOR | VAR_KIND_BODY_ARGUMENTS_ANCHOR
            ))
            || v.scope_level > 65535
        {
            return Err(format!(
                "unsupported eval binding metadata: var_kind {} scope_level {}",
                v.var_kind, v.scope_level
            ));
        }
    }
    if layout.args.iter().any(|v| v.var_kind == VAR_KIND_ARGUMENTS)
        || layout
            .locals
            .iter()
            .filter(|v| v.var_kind == VAR_KIND_ARGUMENTS)
            .count()
            > 1
        || layout.locals.iter().chain(&layout.refs).any(|v| {
            v.var_kind == VAR_KIND_ARGUMENTS
                && (!is_name(v, "arguments")
                    || v.scope_level != 0
                    || v.is_const
                    || v.is_lexical
                    || !v.is_captured)
        })
    {
        return Err("invalid materialized arguments metadata".into());
    }
    let anchors: Vec<_> = layout
        .locals
        .iter()
        .enumerate()
        .filter(|(_, v)| {
            matches!(
                v.var_kind,
                VAR_KIND_BODY_ANCHOR | VAR_KIND_BODY_ARGUMENTS_ANCHOR
            )
        })
        .collect();
    if anchors.len() > 1
        || layout.args.iter().chain(&layout.refs).any(|v| {
            matches!(
                v.var_kind,
                VAR_KIND_BODY_ANCHOR | VAR_KIND_BODY_ARGUMENTS_ANCHOR
            )
        })
        || anchors.iter().any(|(_, v)| {
            !v.name.is_empty()
                || v.scope_level != 2
                || !v.is_lexical
                || !v.is_captured
                || v.is_const
        })
        || layout
            .locals
            .iter()
            .any(|v| v.var_kind == VAR_KIND_ARGUMENTS && !layout.flags.arguments_allowed)
    {
        return Err("invalid split-body anchor metadata".into());
    }
    let body_arguments = layout
        .locals
        .iter()
        .any(|v| v.scope_level == 0 && v.var_kind == 0 && !v.is_lexical && is_name(v, "arguments"));
    if anchors
        .first()
        .is_some_and(|(_, v)| (v.var_kind == VAR_KIND_BODY_ARGUMENTS_ANCHOR) != body_arguments)
    {
        return Err("split-body arguments shadow metadata mismatch".into());
    }
    for v in layout.args.iter().chain(&layout.locals) {
        if v.is_captured && (v.var_ref_idx >= layout.var_ref_count || !refs.insert(v.var_ref_idx)) {
            return Err("eval reserved reference index".into());
        }
        if v.live && !v.is_captured {
            return Err("uncaptured slot cannot provide a live reference".into());
        }
    }
    for (at, v) in layout.locals.iter().enumerate() {
        if v.scope_next
            .local_index()
            .is_some_and(|n| n as usize >= layout.locals.len() || n as usize == at)
        {
            return Err("eval scope link".into());
        }
    }
    let mut marks = vec![0u8; layout.locals.len()];
    for start in 0..marks.len() {
        let mut at = Some(start);
        let mut path = Vec::new();
        while let Some(n) = at {
            if marks[n] == 2 {
                break;
            }
            if marks[n] == 1 {
                return Err("eval scope cycle".into());
            }
            marks[n] = 1;
            path.push(n);
            at = layout.locals[n]
                .scope_next
                .local_index()
                .map(|n| n as usize);
        }
        for n in path {
            marks[n] = 2;
        }
    }
    for (_, anchor) in &anchors {
        let mut next = anchor.scope_next;
        let mut saw_parameter = false;
        while let ScopeLink::Local(n) = next {
            let v = &layout.locals[n as usize];
            saw_parameter |= v.scope_level == 1;
            next = v.scope_next;
        }
        if next != ScopeLink::Parameters || !saw_parameter {
            return Err("body anchor has no retained parameter environment".into());
        }
    }
    let has_split_environment = layout
        .locals
        .iter()
        .any(|v| v.scope_next == ScopeLink::Parameters);
    let mut visible = Vec::new();
    let mut head = layout.scope_head;
    let mut body = false;
    let mut arguments_shadowed = false;
    let mut parameters = ScopeLink::End;
    while let ScopeLink::Local(n) = head {
        let v = &layout.locals[n as usize];
        body |= matches!(
            v.var_kind,
            VAR_KIND_BODY_ANCHOR | VAR_KIND_BODY_ARGUMENTS_ANCHOR
        );
        arguments_shadowed |= v.var_kind == VAR_KIND_BODY_ARGUMENTS_ANCHOR;
        if body && v.scope_level == 1 {
            parameters = head;
            break;
        }
        if v.scope_level > 0 {
            visible.push(VisibleBinding {
                slot: Slot::Local(n),
                binding: v.clone(),
            });
        }
        head = v.scope_next;
    }
    let parameter_scope = has_split_environment && !body;
    if !has_split_environment {
        for (n, v) in layout.args.iter().enumerate() {
            visible.push(VisibleBinding {
                slot: Slot::Arg(n as u32),
                binding: v.clone(),
            });
        }
    }
    for (n, v) in layout.locals.iter().enumerate() {
        if v.scope_level == 0
            && !is_name(v, "<ret>")
            && if body {
                !is_arg_pseudo(v)
            } else {
                !parameter_scope || is_arg_pseudo(v)
            }
        {
            visible.push(VisibleBinding {
                slot: Slot::Local(n as u32),
                binding: v.clone(),
            });
        }
    }
    while let ScopeLink::Local(n) = parameters {
        let v = &layout.locals[n as usize];
        visible.push(VisibleBinding {
            slot: Slot::Local(n),
            binding: v.clone(),
        });
        parameters = v.scope_next;
    }
    if body {
        for (n, v) in layout.locals.iter().enumerate() {
            if v.scope_level == 0
                && is_arg_pseudo(v)
                && !(arguments_shadowed && v.var_kind == VAR_KIND_ARGUMENTS)
            {
                visible.push(VisibleBinding {
                    slot: Slot::Local(n as u32),
                    binding: v.clone(),
                });
            }
        }
    }
    for (n, v) in layout.refs.iter().enumerate() {
        visible.push(VisibleBinding {
            slot: Slot::Ref(n as u32),
            binding: v.clone(),
        })
    }
    let chain = visible.clone();
    let mut names = BTreeSet::new();
    visible.retain(|v| !v.binding.name.is_empty() && names.insert(v.binding.name.clone()));
    let indices = visible
        .iter()
        .enumerate()
        .map(|(at, v)| (v.binding.name.clone(), at))
        .collect();
    let digest = layout_digest(&layout);
    Ok(VerifiedEnvLayout {
        layout,
        digest,
        bindings: visible,
        chain,
        parameter_scope,
        indices,
    })
}
fn layout_digest(l: &EnvLayout) -> [u8; 32] {
    fn num(h: &mut Sha256, v: u32) {
        h.update(v.to_le_bytes())
    }
    fn link(h: &mut Sha256, v: ScopeLink) {
        match v {
            ScopeLink::End => h.update([0]),
            ScopeLink::Parameters => h.update([1]),
            ScopeLink::Local(n) => {
                h.update([2]);
                num(h, n)
            }
        }
    }
    let mut h = Sha256::new();
    h.update(b"jetbunny-eval-layout-v1");
    num(&mut h, l.target_fingerprint.len() as u32);
    h.update(l.target_fingerprint.as_bytes());
    link(&mut h, l.scope_head);
    h.update([l.strict as u8]);
    num(&mut h, l.var_ref_count);
    let f = &l.flags;
    h.update([
        f.has_prototype as u8,
        f.simple_parameters as u8,
        f.derived_constructor as u8,
        f.need_home_object as u8,
        f.kind,
        f.new_target as u8,
        f.super_call as u8,
        f.super_allowed as u8,
        f.arguments_allowed as u8,
        f.backtrace_barrier as u8,
    ]);
    for slots in [&l.args, &l.locals, &l.refs] {
        num(&mut h, slots.len() as u32);
        for v in slots {
            num(&mut h, v.name.len() as u32);
            for c in &v.name {
                h.update(c.to_le_bytes())
            }
            h.update([
                v.is_const as u8,
                v.is_lexical as u8,
                v.var_kind,
                v.is_captured as u8,
                v.initialized as u8,
                v.live as u8,
            ]);
            num(&mut h, v.scope_level);
            link(&mut h, v.scope_next);
            num(&mut h, v.var_ref_idx)
        }
    }
    h.finalize().into()
}
/// A native function graph verified against an immutable external lexical shape.
/// Live-frame authority and lifetime are deliberately not represented here.
#[derive(Clone, Debug)]
pub struct VerifiedEvalImage {
    bytes: Vec<u8>,
    function: FunctionImage,
    layout: VerifiedEnvLayout,
    digest: [u8; 32],
    spans: Option<super::SpanMetadata>,
    profile: &'static str,
}
impl VerifiedEvalImage {
    pub fn serialize_artifact(&self) -> Result<Vec<u8>, ImageError> {
        self.serialize_artifact_with_limits(&super::ImageLimits::default())
    }
    pub fn serialize_artifact_with_limits(
        &self,
        l: &super::ImageLimits,
    ) -> Result<Vec<u8>, ImageError> {
        super::artifact::serialize_eval(self, l)
    }

    pub fn spans(&self) -> Option<&super::SpanMetadata> {
        self.spans.as_ref()
    }
    pub fn profile(&self) -> &'static str {
        self.profile
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn function(&self) -> &FunctionImage {
        &self.function
    }
    pub fn layout(&self) -> &VerifiedEnvLayout {
        &self.layout
    }
    pub fn layout_digest(&self) -> &[u8; 32] {
        self.layout.digest()
    }
    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
    pub fn target_fingerprint(&self) -> &'static str {
        crate::production::TARGET_FINGERPRINT
    }
}
pub fn encode_eval(f: &FunctionImage, layout: &VerifiedEnvLayout) -> Result<Vec<u8>, ImageError> {
    encode_eval_with_limits(f, layout, &super::ImageLimits::default())
}
pub fn encode_eval_with_limits(
    f: &FunctionImage,
    layout: &VerifiedEnvLayout,
    l: &super::ImageLimits,
) -> Result<Vec<u8>, ImageError> {
    super::graph_verify::verify_eval_with_limits(f, layout, l)?;
    super::format::encode_with_limits(f, l)
}
pub fn decode_eval_and_verify(
    bytes: &[u8],
    layout: &VerifiedEnvLayout,
) -> Result<VerifiedEvalImage, ImageError> {
    decode_eval_and_verify_with_limits(bytes, layout, &super::ImageLimits::default())
}
pub fn decode_eval_and_verify_with_limits(
    bytes: &[u8],
    layout: &VerifiedEnvLayout,
    l: &super::ImageLimits,
) -> Result<VerifiedEvalImage, ImageError> {
    Ok(decode_limited(bytes, layout, l)?.0)
}
fn decode_limited(
    bytes: &[u8],
    layout: &VerifiedEnvLayout,
    l: &super::ImageLimits,
) -> Result<(VerifiedEvalImage, usize), ImageError> {
    let (image, used) = super::format::decode_with_limits(bytes, l)?;
    let function = match image {
        Image::Function(f) => f,
        Image::Module(_) => return Err("module is not an eval image".into()),
    };
    let remaining = super::ImageLimits {
        max_work: l.max_work - used,
        ..*l
    };
    let semantic = super::graph_verify::verify_eval_with_limits(&function, layout, &remaining)?;
    Ok((
        VerifiedEvalImage {
            bytes: bytes.to_vec(),
            function,
            layout: layout.clone(),
            digest: Sha256::digest(bytes).into(),
            spans: None,
            profile: super::NATIVE_PROFILE,
        },
        used + semantic,
    ))
}
pub fn verify_and_encode_eval(
    f: &FunctionImage,
    layout: &VerifiedEnvLayout,
    spans: Option<super::SpanMetadata>,
) -> Result<VerifiedEvalImage, ImageError> {
    verify_and_encode_eval_with_limits(f, layout, spans, &super::ImageLimits::default())
}
pub fn verify_and_encode_eval_with_limits(
    f: &FunctionImage,
    layout: &VerifiedEnvLayout,
    spans: Option<super::SpanMetadata>,
    l: &super::ImageLimits,
) -> Result<VerifiedEvalImage, ImageError> {
    let graph_limits = super::artifact::graph_limits(spans.as_ref(), l)?;
    let used = super::graph_verify::verify_eval_with_limits(f, layout, &graph_limits)?;
    let bytes = super::format::encode_with_limits(f, &graph_limits)?;
    let remaining = super::ImageLimits {
        max_work: l.max_work - used,
        ..graph_limits
    };
    let (mut image, used) = decode_limited(&bytes, layout, &remaining)?;
    let remaining = super::ImageLimits {
        max_work: remaining.max_work - used,
        ..*l
    };
    if let Some(meta) = &spans {
        super::artifact::verify_spans_with_limits(image.function(), meta, &remaining)?;
    }
    image.spans = spans;
    image.profile = super::COMPILER_PROFILE;
    Ok(image)
}
pub fn deserialize_eval_artifact(
    bytes: &[u8],
    layout: &VerifiedEnvLayout,
) -> Result<VerifiedEvalImage, ImageError> {
    deserialize_eval_artifact_with_limits(bytes, layout, &super::ImageLimits::default())
}
pub fn deserialize_eval_artifact_with_limits(
    bytes: &[u8],
    layout: &VerifiedEnvLayout,
    l: &super::ImageLimits,
) -> Result<VerifiedEvalImage, ImageError> {
    super::artifact::deserialize_eval(bytes, layout, l)
}
pub(super) fn from_artifact(
    bytes: &[u8],
    layout: &VerifiedEnvLayout,
    spans: Option<super::SpanMetadata>,
    profile: &'static str,
    l: &super::ImageLimits,
    original_limits: &super::ImageLimits,
) -> Result<VerifiedEvalImage, ImageError> {
    let (mut image, used) = decode_limited(bytes, layout, l)?;
    let remaining = super::ImageLimits {
        max_work: l.max_work - used,
        ..*original_limits
    };
    if let Some(meta) = &spans {
        super::artifact::verify_spans_with_limits(image.function(), meta, &remaining)?;
    }
    image.spans = spans;
    image.profile = profile;
    Ok(image)
}

#[cfg(test)]
mod malformed_tests {
    use super::*;
    use crate::production::ids;
    #[test]
    fn malformed_native_capture_records_cannot_bypass_the_external_proof() {
        let env = verify_env_layout(EnvLayout {
            args: vec![EnvBinding {
                name: vec![b'a' as u16],
                is_captured: true,
                initialized: true,
                live: true,
                ..Default::default()
            }],
            var_ref_count: 1,
            ..Default::default()
        })
        .unwrap();
        let mut f = FunctionImage {
            code: vec![ids::get_var_ref, 0, 0, ids::r#return],
            stack_size: 1,
            captures: vec![Capture {
                name: vec![b'a' as u16],
                closure_type: 1,
                ..Default::default()
            }],
            ..Default::default()
        };
        let valid = super::super::format::encode(&f).unwrap();
        decode_eval_and_verify(&valid, &env).unwrap();
        for end in 0..valid.len() {
            assert!(decode_eval_and_verify(&valid[..end], &env).is_err());
        }
        f.captures[0].var_idx = 1;
        let bad = super::super::format::encode(&f).unwrap();
        assert!(decode_eval_and_verify(&bad, &env).is_err());
        f.captures[0].var_idx = 0;
        f.captures[0].closure_type = 7;
        assert!(decode_eval_and_verify(&super::super::format::encode(&f).unwrap(), &env).is_err());
    }
}
