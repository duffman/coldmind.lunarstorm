//! Tighten-only native representation and verifier resource policy.
use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageLimits {
    pub max_native_bytes: usize,
    pub max_graph_bytes: usize,
    pub max_functions: usize,
    /// Root depth is zero: this bounds nested function edges.
    pub max_depth: usize,
    pub max_table_entries: usize,
    pub max_instructions: usize,
    pub max_positions: usize,
    pub max_state_cells: usize,
    pub max_work: usize,
    pub max_artifact_bytes: usize,
}
impl Default for ImageLimits {
    fn default() -> Self {
        Self {
            // Whole-image caps sized for real bundles; per-function native
            // tables stay at 16-bit bounds (max_table_entries).
            max_native_bytes: 256 * 1024 * 1024,
            max_graph_bytes: 1024 * 1024 * 1024,
            max_functions: 1_000_000,
            max_depth: 256,
            max_table_entries: 65535,
            max_instructions: 32_000_000,
            max_positions: 32_000_000,
            max_state_cells: 256_000_000,
            max_work: 2_000_000_000,
            max_artifact_bytes: 1024 * 1024 * 1024,
        }
    }
}
/// Most instructions one function may have, independent of the whole-image total above.
///
/// Decision: when the whole-image caps were raised so complete bundles fit (the Rust compiler
/// became the only compiler), the per-function bounds must not follow them. One function's
/// verifier state is instructions × (locals + 1) bytes and locals reach the u16 maximum
/// (65535), so an unbounded function could make the verifier allocate gigabytes.
pub const MAX_FUNCTION_INSTRUCTIONS: usize = 1_000_000;
/// Most verifier state cells (instructions × (locals + 1)) one function may need; see
/// [`MAX_FUNCTION_INSTRUCTIONS`] for why this does not grow with the image caps.
pub const MAX_FUNCTION_STATE_CELLS: usize = 8_000_000;
impl ImageLimits {
    /// The verifier state cell cap for one function.
    ///
    /// # Returns
    /// The configured whole-image cap, tightened to [`MAX_FUNCTION_STATE_CELLS`].
    pub(super) fn function_state_cells(&self) -> usize {
        self.max_state_cells.min(MAX_FUNCTION_STATE_CELLS)
    }
    pub fn validate(&self) -> Result<(), String> {
        let d = Self::default();
        if self.max_native_bytes > d.max_native_bytes
            || self.max_graph_bytes > d.max_graph_bytes
            || self.max_functions > d.max_functions
            || self.max_depth > d.max_depth
            || self.max_table_entries > d.max_table_entries
            || self.max_instructions > d.max_instructions
            || self.max_positions > d.max_positions
            || self.max_state_cells > d.max_state_cells
            || self.max_work > d.max_work
            || self.max_artifact_bytes > d.max_artifact_bytes
        {
            return Err("capacity: image limits may only tighten defaults".into());
        }
        Ok(())
    }
    /// Limits for decoding one function's code.
    ///
    /// # Returns
    /// The native byte cap, and the instruction cap tightened to
    /// [`MAX_FUNCTION_INSTRUCTIONS`].
    pub(super) fn code(&self) -> crate::Limits {
        crate::Limits {
            max_bytes: self.max_native_bytes,
            max_instructions: self.max_instructions.min(MAX_FUNCTION_INSTRUCTIONS),
        }
    }
}
pub(super) struct Work {
    remaining: usize,
}
impl Work {
    pub(super) fn new(limits: &ImageLimits) -> Self {
        Self {
            remaining: limits.max_work,
        }
    }
    pub(super) fn remaining(&self) -> usize {
        self.remaining
    }
    pub(super) fn charge(&mut self, n: usize) -> Result<(), String> {
        self.remaining = self
            .remaining
            .checked_sub(n)
            .ok_or("capacity: image verification work budget")?;
        Ok(())
    }
}
/// Scan widths/counts without allocating decoded instruction/state vectors.
pub(super) fn instructions(
    code: &[u8],
    limits: &ImageLimits,
    work: &mut Work,
) -> Result<usize, String> {
    if code.len() > limits.max_native_bytes {
        return Err("capacity: native code byte budget".into());
    }
    let mut offset = 0;
    let mut count = 0;
    while offset < code.len() {
        work.charge(1)?;
        if count == limits.max_instructions.min(MAX_FUNCTION_INSTRUCTIONS) {
            return Err("capacity: native instruction budget".into());
        }
        let id = code[offset];
        let op = id
            .checked_sub(1)
            .and_then(|n| crate::production::FINAL_OPCODES.get(n as usize))
            .ok_or("invalid final opcode")?;
        let end = offset
            .checked_add(op.width as usize)
            .ok_or("capacity: instruction length overflow")?;
        if end > code.len() {
            return Err("truncated final instruction".into());
        }
        offset = end;
        count += 1;
    }
    Ok(count)
}
pub(super) fn preflight(
    root: &FunctionImage,
    module: Option<&ModuleImage>,
    limits: &ImageLimits,
) -> Result<Work, String> {
    limits.validate()?;
    let mut bytes = limits.max_graph_bytes;
    let mut work = Work::new(limits);
    let mut charge = |n: usize| -> Result<(), String> {
        bytes = bytes
            .checked_sub(n)
            .ok_or("capacity: image graph allocation budget")?;
        Ok(())
    };
    if let Some(m) = module {
        for n in [
            m.required_modules.len(),
            m.exports.len(),
            m.star_exports.len(),
            m.imports.len(),
        ] {
            if n > limits.max_table_entries {
                return Err("capacity: module table budget".into());
            }
        }
        charge(
            m.required_modules.len() * std::mem::size_of::<Vec<u16>>()
                + m.exports.len() * std::mem::size_of::<ModuleExport>()
                + m.imports.len() * std::mem::size_of::<ModuleImport>()
                + m.star_exports.len() * 4,
        )?;
        // lunarstorm patch: attribute keys and values count as module text.
        let texts = std::iter::once(&m.name)
            .chain(m.required_modules.iter())
            .chain(m.required_attributes.iter().flatten().flat_map(|(k, v)| [k, v]))
            .chain(m.imports.iter().map(|v| &v.name))
            .chain(
                m.exports
                    .iter()
                    .flat_map(|v| match v {
                        ModuleExport::Local { name, .. } => [Some(name), None],
                        ModuleExport::Indirect { import, name, .. } => [Some(name), Some(import)],
                    })
                    .flatten(),
            );
        for t in texts {
            charge(t.len().saturating_mul(2))?;
            work.charge(t.len() + 1)?;
        }
    }
    let mut pending = vec![(root, 0usize)];
    let mut functions = 0usize;
    let mut cells = 0usize;
    let mut code_bytes = 0usize;
    while let Some((f, depth)) = pending.pop() {
        work.charge(1)?;
        functions += 1;
        if functions > limits.max_functions || depth > limits.max_depth {
            return Err("capacity: function graph budget".into());
        }
        let max = limits.max_table_entries;
        if f.arg_count as usize > max
            || f.local_count as usize > max
            || f.defined_arg_count as usize > max
            || f.stack_size as usize > max
            || f.var_ref_count as usize > max
            || f.constants.len() > max
            || f.captures.len() > max
            || f.atoms.len() > max
            || f.locals.len() > f.arg_count as usize + f.local_count as usize
        {
            return Err("capacity: function table representation budget".into());
        }
        if f.debug_locations.len() > limits.max_positions {
            return Err("capacity: native location count budget".into());
        }
        charge(
            std::mem::size_of::<FunctionImage>()
                + f.code.len()
                + f.function_name.len()
                + f.filename.len()
                + f.locals.len() * std::mem::size_of::<VarDef>()
                + f.captures.len() * std::mem::size_of::<Capture>()
                + f.constants.len() * std::mem::size_of::<Constant>()
                + f.atoms.len() * std::mem::size_of::<Vec<u16>>()
                + f.debug_locations.len() * std::mem::size_of::<DebugLocation>(),
        )?;
        code_bytes = code_bytes
            .checked_add(f.code.len())
            .ok_or("capacity: code graph length overflow")?;
        if code_bytes > limits.max_native_bytes {
            return Err("capacity: aggregate native code byte budget".into());
        }
        let count = instructions(&f.code, limits, &mut work)?;
        cells = cells
            .checked_add(
                count
                    .checked_mul(f.local_count as usize + 1)
                    .ok_or("capacity: state cell overflow")?,
            )
            .ok_or("capacity: state cell overflow")?;
        if cells > limits.max_state_cells {
            return Err("capacity: aggregate verifier state cell budget".into());
        }
        work.charge(
            f.function_name.len()
                + f.filename.len()
                + f.locals.len()
                + f.captures.len()
                + f.debug_locations.len()
                + f.constants.len()
                + 1,
        )?;
        for t in f
            .atoms
            .iter()
            .chain(f.locals.iter().map(|v| &v.name))
            .chain(f.captures.iter().map(|v| &v.name))
            .chain(f.source.iter())
            .chain(f.constants.iter().filter_map(|v| {
                if let Constant::String(s) = v {
                    Some(s)
                } else {
                    None
                }
            }))
        {
            if t.len() > 8 * 1024 * 1024 {
                return Err("capacity: native string budget".into());
            }
            charge(t.len().saturating_mul(2))?;
            work.charge(t.len() + 1)?;
        }
        for c in &f.constants {
            match c {
                Constant::BigInt(b) => {
                    if b.len() > 1024 * 1024 {
                        return Err("capacity: BigInt byte budget".into());
                    }
                    charge(b.len())?;
                    work.charge(b.len() + 1)?;
                }
                Constant::TemplateObject { cooked, raw } => {
                    for t in raw.iter().chain(cooked.iter().flatten()) {
                        charge(t.len().saturating_mul(2) + 16)?;
                        work.charge(t.len() + 1)?;
                    }
                }
                Constant::Function(child) => {
                    if functions + pending.len() >= limits.max_functions
                        || depth == limits.max_depth
                    {
                        return Err("capacity: function graph budget".into());
                    }
                    pending.push((child, depth + 1))
                }
                _ => {}
            }
        }
    }
    Ok(work)
}

pub(super) fn decode_error(error: crate::Error) -> String {
    match error {
        crate::Error::ByteLimitExceeded { .. }
        | crate::Error::InstructionLimitExceeded { .. }
        | crate::Error::AllocationFailed => format!("capacity: {error}"),
        _ => error.to_string(),
    }
}
