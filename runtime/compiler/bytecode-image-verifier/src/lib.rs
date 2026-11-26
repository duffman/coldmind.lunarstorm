//! Bounded decoding and structural branch checks for the pinned QuickJS-NG
//! **final raw instruction buffer**, not a serialized `JS_WriteObject` object.
//! Multi-byte operands use the running host's native byte order, matching
//! upstream `get_u16`/`get_u32`. Buffers from a different byte-order host must be
//! converted before inspection; this crate does not identify their provenance.
//!
//! Opcode identities, widths, formats, and declared stack effects are generated
//! by compiling the vendored upstream opcode header. Only its final `DEF`
//! instructions are accepted; `invalid` and unknown identities are rejected.
//! Temporary `def` identities overlap valid final short identities, so bytes
//! alone cannot establish compiler phase. A temporary instruction with such an
//! identity is necessarily interpreted as its final short counterpart.
//!
//! Structural validation checks every encoded branch, including unreachable
//! instructions. It does **not** verify stack semantics, exception/finally
//! discipline, operand indices, object serialization, or runtime safety. A
//! successful result must not be used to execute untrusted QuickJS bytecode.
//! In particular, the dynamic target of `ret` is outside this validation scope.

use std::fmt;

/// Upstream source revision whose opcode header defines this crate's schema.
pub const UPSTREAM_REVISION: &str = "e22ce4de4362dc5fd4794cfa3c03fc7fdba5f015";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Opcode {
    pub id: u8,
    pub name: &'static str,
    pub width: u8,
    /// Upstream's declared base pop count. `npop`/`npop_u16`/`npopx`
    /// additionally pop a number of arguments encoded in the operand or opcode.
    pub pop: u8,
    /// Upstream's declared push count; this is metadata, not a semantic proof.
    pub push: u8,
    pub format: OperandFormat,
}

include!(concat!(env!("OUT_DIR"), "/opcodes.rs"));

/// Bounds applied before and during decoding. Set both for the caller's budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    pub max_bytes: usize,
    pub max_instructions: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024 * 1024,
            max_instructions: 1_000_000,
        }
    }
}

/// Machine-readable scope of this crate's checks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capabilities {
    pub structural_branch_validation: bool,
    pub stack_semantic_validation: bool,
    pub compiler_phase_provenance: bool,
    pub runtime_safety_validation: bool,
}
pub const CAPABILITIES: Capabilities = Capabilities {
    structural_branch_validation: true,
    stack_semantic_validation: false,
    compiler_phase_provenance: false,
    runtime_safety_validation: false,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    ByteLimitExceeded {
        length: usize,
        maximum: usize,
    },
    InstructionLimitExceeded {
        offset: usize,
        maximum: usize,
    },
    InvalidOpcode {
        offset: usize,
        opcode: u8,
    },
    UnknownOpcode {
        offset: usize,
        opcode: u8,
    },
    TruncatedInstruction {
        offset: usize,
        opcode: u8,
        expected: usize,
        available: usize,
    },
    BranchOutOfRange {
        offset: usize,
        target: i128,
        length: usize,
    },
    BranchNotInstructionBoundary {
        offset: usize,
        target: usize,
    },
    AllocationFailed,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ByteLimitExceeded { length, maximum } => write!(f, "buffer length {length} exceeds byte limit {maximum}"),
            Self::InstructionLimitExceeded { offset, maximum } => write!(f, "instruction at offset {offset} exceeds instruction limit {maximum}"),
            Self::InvalidOpcode { offset, opcode } => write!(f, "invalid opcode {opcode} at offset {offset}"),
            Self::UnknownOpcode { offset, opcode } => write!(f, "unknown final opcode {opcode} at offset {offset}"),
            Self::TruncatedInstruction { offset, opcode, expected, available } => write!(f, "truncated opcode {opcode} at offset {offset}: needs {expected} bytes, has {available}"),
            Self::BranchOutOfRange { offset, target, length } => write!(f, "branch at offset {offset} targets {target}, outside buffer 0..{length}"),
            Self::BranchNotInstructionBoundary { offset, target } => write!(f, "branch at offset {offset} targets operand byte {target}"),
            Self::AllocationFailed => f.write_str("could not allocate bounded instruction metadata"),
        }
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug)]
pub struct Instruction<'a> {
    offset: usize,
    opcode: &'static Opcode,
    bytes: &'a [u8],
}
impl<'a> Instruction<'a> {
    pub fn offset(&self) -> usize {
        self.offset
    }
    pub fn opcode(&self) -> &'static Opcode {
        self.opcode
    }
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
    pub fn operands(&self) -> &'a [u8] {
        &self.bytes[1..]
    }
    /// Encoded relative branch destination. This can be outside the buffer
    /// until `validate_structure` has checked it. The base is the address of
    /// the displacement operand: opcode + 1, or opcode + 5 after an atom.
    /// No target is reported for `ret`, whose destination is a runtime value.
    pub fn branch_target(&self) -> Option<i128> {
        let (operand_offset, displacement) = match self.opcode.format {
            OperandFormat::Label8 => (1, i128::from(self.bytes[1] as i8)),
            OperandFormat::Label16 => (
                1,
                i128::from(i16::from_ne_bytes([self.bytes[1], self.bytes[2]])),
            ),
            OperandFormat::Label | OperandFormat::LabelU16 => (1, self.displacement32(1)),
            OperandFormat::AtomLabelU8 | OperandFormat::AtomLabelU16 => (5, self.displacement32(5)),
            _ => return None,
        };
        Some(self.offset as i128 + operand_offset + displacement)
    }

    fn displacement32(&self, offset: usize) -> i128 {
        i128::from(i32::from_ne_bytes([
            self.bytes[offset],
            self.bytes[offset + 1],
            self.bytes[offset + 2],
            self.bytes[offset + 3],
        ]))
    }
}

#[derive(Debug)]
pub struct DecodedBytecode<'a> {
    instructions: Vec<Instruction<'a>>,
}
impl<'a> DecodedBytecode<'a> {
    pub fn instructions(&self) -> &[Instruction<'a>] {
        &self.instructions
    }
}
/// Decode all instructions with bounded input and instruction counts.
///
/// Operand slices borrow `bytes`; only instruction metadata is allocated.
/// This checks identities and widths, but does not check branch destinations.
/// An empty instruction buffer is accepted as structurally empty.
pub fn decode(bytes: &[u8], limits: Limits) -> Result<DecodedBytecode<'_>, Error> {
    if bytes.len() > limits.max_bytes {
        return Err(Error::ByteLimitExceeded {
            length: bytes.len(),
            maximum: limits.max_bytes,
        });
    }
    decode_schema(bytes, limits, FINAL_OPCODES)
}
pub fn decode_schema<'a>(
    bytes: &'a [u8],
    limits: Limits,
    schema: &'static [Opcode],
) -> Result<DecodedBytecode<'a>, Error> {
    if bytes.len() > limits.max_bytes {
        return Err(Error::ByteLimitExceeded {
            length: bytes.len(),
            maximum: limits.max_bytes,
        });
    }
    let mut instructions = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        if instructions.len() == limits.max_instructions {
            return Err(Error::InstructionLimitExceeded {
                offset,
                maximum: limits.max_instructions,
            });
        }
        let id = bytes[offset];
        if id == ids::invalid {
            return Err(Error::InvalidOpcode { offset, opcode: id });
        }
        // Final DEF enumeration is contiguous starting at invalid == 0. The
        // generated table excludes invalid; temporary def entries never enter it.
        let opcode = schema
            .get(usize::from(id) - 1)
            .ok_or(Error::UnknownOpcode { offset, opcode: id })?;
        let width = usize::from(opcode.width);
        let remaining = bytes.len() - offset;
        if width > remaining {
            return Err(Error::TruncatedInstruction {
                offset,
                opcode: id,
                expected: width,
                available: remaining,
            });
        }
        if instructions.len() == instructions.capacity() {
            // Reserve in bounded chunks. Fallible reservation avoids an
            // allocation panic on a caller-selected large budget.
            let additional = (limits.max_instructions - instructions.len())
                .min(remaining)
                .min(1024);
            instructions
                .try_reserve_exact(additional)
                .map_err(|_| Error::AllocationFailed)?;
        }
        instructions.push(Instruction {
            offset,
            opcode,
            bytes: &bytes[offset..offset + width],
        });
        offset += width;
    }
    Ok(DecodedBytecode { instructions })
}

/// Decode, then require all encoded branch destinations to be in the buffer
/// and exactly at a decoded instruction boundary. A target at `bytes.len()` is
/// rejected. No stack, index, exception discipline, or VM safety proof is made.
pub fn validate_structure(bytes: &[u8], limits: Limits) -> Result<DecodedBytecode<'_>, Error> {
    validate_structure_schema(bytes, limits, FINAL_OPCODES)
}
pub fn validate_structure_schema<'a>(
    bytes: &'a [u8],
    limits: Limits,
    schema: &'static [Opcode],
) -> Result<DecodedBytecode<'a>, Error> {
    let decoded = decode_schema(bytes, limits, schema)?;
    for instruction in &decoded.instructions {
        let Some(target) = instruction.branch_target() else {
            continue;
        };
        if target < 0 || target >= bytes.len() as i128 {
            return Err(Error::BranchOutOfRange {
                offset: instruction.offset,
                target,
                length: bytes.len(),
            });
        }
        let target = target as usize;
        if decoded
            .instructions
            .binary_search_by_key(&target, |instruction| instruction.offset)
            .is_err()
        {
            return Err(Error::BranchNotInstructionBoundary {
                offset: instruction.offset,
                target,
            });
        }
    }
    Ok(decoded)
}

/// Owned final bytecode snapshots.
#[path = "instruction_snapshot.rs"]
pub mod snapshot;
/// Independent scalar semantic verifier.
#[path = "scalar_subset_checks.rs"]
pub mod verify;

pub mod production {
    use super::{Opcode, OperandFormat};
    include!(concat!(env!("OUT_DIR"), "/production.rs"));
    pub const UPSTREAM_REVISION: &str = "7b5f588e0f9b643e2996f291bfc861abeaceddc2";
    pub const TARGET_FINGERPRINT: &str = env!("QJS_PRODUCTION_FINGERPRINT");
}
#[path = "native_function_graph_image.rs"]
pub mod image;

/// Rust-owned backend primitives for the pinned native regexp execution VM.
#[path = "regexp_bytecode_compiler.rs"]
pub mod regexp;
