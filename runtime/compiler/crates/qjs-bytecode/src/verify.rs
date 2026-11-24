//! Independent scalar-subset CFG, stack, index and definite-assignment checks.
use crate::{snapshot::*, validate_structure, Instruction, Limits};
use std::collections::{BTreeMap, VecDeque};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationError {
    pub offset: Option<usize>,
    pub message: String,
}
impl std::fmt::Display for VerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "verification at {:?}: {}", self.offset, self.message)
    }
}
impl std::error::Error for VerificationError {}
/// Proof token whose construction is private to this independent verifier.
#[derive(Debug)]
pub struct VerifiedSnapshot(Snapshot);
impl VerifiedSnapshot {
    pub fn snapshot(&self) -> &Snapshot {
        &self.0
    }
    pub fn into_snapshot(self) -> Snapshot {
        self.0
    }
}
fn error(offset: Option<usize>, message: impl Into<String>) -> VerificationError {
    VerificationError {
        offset,
        message: message.into(),
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    depth: usize,
    initialized: Vec<bool>,
}
// This model deliberately does not use opcode.pop/push (the engine schema).
fn effects(name: &str) -> Option<(usize, usize)> {
    match name {
        "push_i32" | "push_i16" | "push_i8" | "push_minus1" | "push_0" | "push_1" | "push_2"
        | "push_3" | "push_4" | "push_5" | "push_6" | "push_7" | "push_const" | "push_const8"
        | "push_true" | "push_false" | "get_loc" | "get_loc8" | "get_loc0" | "get_loc1"
        | "get_loc2" | "get_loc3" => Some((0, 1)),
        "get_loc0_loc1" => Some((0, 2)),
        "put_loc" | "put_loc8" | "put_loc0" | "put_loc1" | "put_loc2" | "put_loc3" | "return"
        | "if_false" | "if_false8" | "if_true" | "if_true8" | "drop" => Some((1, 0)),
        "set_loc" | "set_loc8" | "set_loc0" | "set_loc1" | "set_loc2" | "set_loc3" | "neg"
        | "lnot" => Some((1, 1)),
        "mul" | "div" | "mod" | "add" | "sub" | "strict_eq" | "strict_neq" | "lt" | "lte"
        | "gt" | "gte" => Some((2, 1)),
        "goto" | "goto8" | "goto16" | "nop" => Some((0, 0)),
        "dup" => Some((1, 2)),
        _ => None,
    }
}
fn local_indices(i: &Instruction<'_>) -> Vec<usize> {
    let n = i.opcode().name;
    if n == "get_loc0_loc1" {
        return vec![0, 1];
    }
    if !(n.starts_with("get_loc") || n.starts_with("put_loc") || n.starts_with("set_loc")) {
        return vec![];
    }
    let bytes = i.operands();
    vec![if n.ends_with("loc8") {
        bytes[0] as usize
    } else if n.ends_with("_loc") {
        u16::from_ne_bytes([bytes[0], bytes[1]]) as usize
    } else {
        n.as_bytes()[n.len() - 1] as usize - b'0' as usize
    }]
}
/// Verify one complete scalar entry object. Nested-function constants are excluded by the snapshot type.
pub fn verify(
    snapshot: Snapshot,
    expected_fingerprint: &str,
) -> Result<VerifiedSnapshot, VerificationError> {
    verify_impl(snapshot, expected_fingerprint, false)
}
pub fn verify_production(
    snapshot: Snapshot,
    expected_fingerprint: &str,
) -> Result<VerifiedSnapshot, VerificationError> {
    verify_impl(snapshot, expected_fingerprint, true)
}
fn verify_impl(
    snapshot: Snapshot,
    expected_fingerprint: &str,
    production: bool,
) -> Result<VerifiedSnapshot, VerificationError> {
    if snapshot.provenance
        != if production {
            Provenance::RustFinalized
        } else {
            Provenance::QuickJsFinalized
        }
        || snapshot.target_fingerprint != expected_fingerprint
    {
        return Err(error(None, "target or finalization provenance mismatch"));
    }
    if snapshot.function_name != "main"
        || snapshot.arg_count != 0
        || snapshot.local_count > 65535
        || snapshot.constants.len() > 65535
        || snapshot.stack_size > 65535
    {
        return Err(error(None, "entry metadata or table budget invalid"));
    }
    let decoded = if production {
        crate::validate_structure_schema(
            &snapshot.code,
            Limits::default(),
            crate::production::FINAL_OPCODES,
        )
    } else {
        validate_structure(&snapshot.code, Limits::default())
    }
    .map_err(|e| error(None, e.to_string()))?;
    let ins = decoded.instructions();
    // Bound the dataflow matrix before allocating per-instruction local sets.
    if ins.len().saturating_mul(snapshot.local_count as usize + 1) > 8_000_000
        || snapshot.positions.len() > 1_000_000
    {
        return Err(error(None, "verification state budget exceeded"));
    }
    if ins.is_empty() {
        return Err(error(None, "empty entry"));
    }
    let offsets: BTreeMap<usize, usize> = ins
        .iter()
        .enumerate()
        .map(|(n, i)| (i.offset(), n))
        .collect();
    for p in &snapshot.positions {
        if !offsets.contains_key(&(p.code_offset as usize))
            || p.source_start > p.source_end
            || p.source_end > snapshot.source_len
        {
            return Err(error(None, "invalid source position"));
        }
    }
    for i in ins {
        let n = i.opcode().name;
        if effects(n).is_none() {
            return Err(error(
                Some(i.offset()),
                format!("opcode {n} is outside scalar whitelist"),
            ));
        }
        if local_indices(i)
            .iter()
            .any(|&x| x >= snapshot.local_count as usize)
        {
            return Err(error(Some(i.offset()), "local index out of bounds"));
        }
        if n == "push_const" || n == "push_const8" {
            let b = i.operands();
            let index = if n == "push_const8" {
                b[0] as usize
            } else {
                u32::from_ne_bytes([b[0], b[1], b[2], b[3]]) as usize
            };
            if index >= snapshot.constants.len() {
                return Err(error(Some(i.offset()), "constant index out of bounds"));
            }
        }
    }
    let mut states: Vec<Option<State>> = vec![None; ins.len()];
    states[0] = Some(State {
        depth: 0,
        initialized: vec![false; snapshot.local_count as usize],
    });
    let mut work = VecDeque::from([0]);
    while let Some(index) = work.pop_front() {
        let i = &ins[index];
        let n = i.opcode().name;
        let mut state = states[index].clone().expect("queued state");
        let (pop, push) = effects(n).expect("whitelisted");
        if state.depth < pop {
            return Err(error(Some(i.offset()), "stack underflow"));
        }
        if n.starts_with("get_loc") && local_indices(i).iter().any(|&x| !state.initialized[x]) {
            return Err(error(Some(i.offset()), "uninitialized local read"));
        }
        for x in local_indices(i) {
            if n.starts_with("put_loc") || n.starts_with("set_loc") {
                state.initialized[x] = true
            }
        }
        state.depth = state.depth - pop + push;
        if state.depth > snapshot.stack_size as usize {
            return Err(error(Some(i.offset()), "declared stack capacity exceeded"));
        }
        if n == "return" {
            if state.depth != 0 {
                return Err(error(Some(i.offset()), "return leaves stack values"));
            }
            continue;
        }
        let mut successors = vec![];
        if let Some(target) = i.branch_target() {
            successors.push(offsets[&(target as usize)])
        }
        if !n.starts_with("goto") {
            if index + 1 == ins.len() {
                return Err(error(Some(i.offset()), "fallthrough outside function"));
            }
            successors.push(index + 1)
        }
        for next in successors {
            match &mut states[next] {
                None => {
                    states[next] = Some(state.clone());
                    work.push_back(next)
                }
                Some(old) => {
                    if old.depth != state.depth {
                        return Err(error(
                            Some(ins[next].offset()),
                            "inconsistent stack at join",
                        ));
                    }
                    let mut changed = false;
                    for (a, b) in old.initialized.iter_mut().zip(&state.initialized) {
                        if *a && !*b {
                            *a = false;
                            changed = true
                        }
                    }
                    if changed {
                        work.push_back(next)
                    }
                }
            }
        }
    }
    Ok(VerifiedSnapshot(snapshot))
}
