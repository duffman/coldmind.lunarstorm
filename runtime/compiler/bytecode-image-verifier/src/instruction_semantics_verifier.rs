//! Independent instruction semantics: never trust generated opcode stack counts.
use super::*;
use crate::{Instruction, OperandFormat};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

fn effect(i: &Instruction<'_>) -> Result<(usize, usize), String> {
    let n = i.opcode().name;
    let argc = || usize::from(u16::from_le_bytes(i.operands()[..2].try_into().unwrap()));
    let e = match n {
        "push_i32" | "push_i16" | "push_i8" | "push_minus1" | "push_0" | "push_1" | "push_2"
        | "push_3" | "push_4" | "push_5" | "push_6" | "push_7" | "push_const" | "push_const8"
        | "push_atom_value" | "undefined" | "null" | "push_this" | "push_false" | "push_true"
        | "object" | "fclosure" | "fclosure8" | "get_var" | "get_var_undef" | "delete_var" => {
            (0, 1)
        }
        "get_loc0_loc1" => (0, 2),
        "catch" | "rest" => (0, 1),
        "for_in_start" => (1, 1),
        "for_of_start" | "for_await_of_start" | "for_in_next" => (1, 3),
        "await" => (1, 1),
        "yield" | "yield_star" | "async_yield_star" | "iterator_get_value_done" => (1, 2),
        "initial_yield" => (0, 0),
        "iterator_next" => (4, 4),
        "iterator_call" => (4, 5),
        "for_of_next" => (3, 5),
        "for_of_skip" => (3, 4),
        "iterator_close" => (3, 0),
        "special_object" if i.operands()[0] <= 5 || i.operands()[0] == 6 => (0, 1),
        "with_get_var" | "with_delete_var" | "with_get_ref" | "with_make_ref"
            if i.operands()[8] <= 1 =>
        {
            (1, 0)
        }
        "with_put_var" if i.operands()[8] <= 1 => (2, 1),
        // `using`: [value] -> [value, method]; disposal threads an error state on the stack
        // and reads its value/method pair from two anonymous locals (checked below).
        "using_check" if i.operands()[0] <= 1 => (1, 2),
        "using_dispose_init" | "using_dispose_async" => (0, 1),
        "using_dispose" => (1, 1),
        "using_dispose_merge" => (2, 1),
        "using_dispose_end" => (1, 0),
        "get_loc" | "get_loc8" | "get_loc0" | "get_loc1" | "get_loc2" | "get_loc3"
        | "get_loc_check" | "get_arg" | "get_arg0" | "get_arg1" | "get_arg2" | "get_arg3"
        | "get_var_ref" | "get_var_ref0" | "get_var_ref1" | "get_var_ref2" | "get_var_ref3"
        | "get_var_ref_check" => (0, 1),
        "put_loc"
        | "put_loc8"
        | "put_loc0"
        | "put_loc1"
        | "put_loc2"
        | "put_loc3"
        | "put_loc_check"
        | "put_loc_check_init"
        | "put_arg"
        | "put_arg0"
        | "put_arg1"
        | "put_arg2"
        | "put_arg3"
        | "put_var_ref"
        | "put_var_ref0"
        | "put_var_ref1"
        | "put_var_ref2"
        | "put_var_ref3"
        | "put_var_ref_check"
        | "put_var_ref_check_init"
        | "put_var"
        | "put_var_init"
        | "define_func"
        | "drop"
        | "return"
        | "return_async"
        | "throw"
        | "if_false"
        | "if_true"
        | "if_false8"
        | "if_true8" => (1, 0),
        "set_loc"
        | "set_loc8"
        | "set_loc0"
        | "set_loc1"
        | "set_loc2"
        | "set_loc3"
        | "set_arg"
        | "set_arg0"
        | "set_arg1"
        | "set_arg2"
        | "set_arg3"
        | "set_var_ref"
        | "set_var_ref0"
        | "set_var_ref1"
        | "set_var_ref2"
        | "set_var_ref3"
        | "get_field"
        | "set_name"
        | "neg"
        | "plus"
        | "not"
        | "lnot"
        | "typeof"
        | "inc"
        | "dec"
        | "to_object"
        | "to_propkey"
        | "check_object"
        | "is_undefined"
        | "is_undefined_or_null" => (1, 1),
        "mul" | "div" | "mod" | "add" | "sub" | "pow" | "strict_eq" | "strict_neq" | "eq"
        | "neq" | "lt" | "lte" | "gt" | "gte" | "shl" | "sar" | "shr" | "and" | "or" | "xor"
        | "in" | "instanceof" | "get_array_el" | "define_field" | "set_proto" | "delete" => (2, 1),
        "goto"
        | "goto8"
        | "goto16"
        | "nop"
        | "return_undef"
        | "throw_error"
        | "define_var"
        | "check_define_var"
        | "set_loc_uninitialized"
        | "close_loc"
        | "debug_stmt" => (0, 0),
        "dup" | "post_inc" | "post_dec" => (1, 2),
        "nip" => (2, 1),
        "nip1" => (3, 2),
        "dup1" | "insert2" => (2, 3),
        "dup2" => (2, 4),
        "dup3" => (3, 6),
        "insert3" => (3, 4),
        "insert4" => (4, 5),
        "swap" | "set_name_computed" | "get_array_el2" | "to_propkey2" => (2, 2),
        "swap2" | "perm4" | "rot4l" => (4, 4),
        "rot3l" | "rot3r" | "perm3" => (3, 3),
        "rot5l" | "perm5" => (5, 5),
        "get_field2" => (1, 2),
        "put_field" => (2, 0),
        "put_array_el" => (3, 0),
        // [object, name, value]: SetMutableBinding of a with object found by with_make_ref.
        "put_ref_value" => (3, 0),
        "define_array_el" => (3, 2),
        // Object-literal definitions are enumerable. Low bits select a value,
        // getter or setter (0/1/2); 3 and other flag bits are not modeled.
        "define_method" if matches!(i.operands()[4], 0..=2 | 4..=6) => (2, 1),
        "define_method_computed" if matches!(i.operands()[0], 0..=2 | 4..=6) => (3, 1),
        // This operand selects live stack slots, unlike a normal u8 immediate:
        // target=-3, source=-2, exclusion=-1. Arbitrary masks can read deeper
        // than the advertised three slots and need a separate proof.
        "copy_data_properties" if i.operands()[0] == 6 => (3, 3),
        "define_class" if i.operands()[4] <= 1 => (2, 2),
        "define_class_computed" if i.operands()[4] <= 1 => (3, 3),
        "set_home_object" => (2, 2),
        "check_ctor" => (0, 0),
        "init_ctor" => (0, 1),
        "check_ctor_return" => (1, 2),
        "get_super" => (1, 1),
        "get_super_value" => (3, 1),
        "put_super_value" => (4, 0),
        "strict_op" if i.operands()[4] == 0 => (3, 0),
        "strict_op" if i.operands()[4] == 1 => (2, 1),
        "strict_op" if i.operands()[4] == 2 => (4, 0),
        "strict_op" if i.operands()[4] == 3 => (1, 0),
        // Selector 4: regexp literal, pattern and flags strings to a RegExp.
        "strict_op" if i.operands()[4] == 4 => (2, 1),
        // Selector 5 resumes a compiler-generated finally throw completion.
        "strict_op" if i.operands()[4] == 5 => (2, 0),
        "strict_op" if i.operands()[4] == 6 => (0, 1),
        "import" => (2, 1),
        "private_symbol" => (0, 1),
        "get_private_field" | "private_in" => (2, 1),
        "put_private_field" => (3, 0),
        "define_private_field" => (3, 1),
        "check_brand" => (2, 2),
        "add_brand" => (2, 0),
        "apply" if argc() <= 1 => (3, 1),
        "call" => (argc() + 1, 1),
        // Direct eval: func args... / func array -> result.
        "eval" => (argc() + 1, 1),
        "apply_eval" => (2, 1),
        "call_method" | "call_constructor" => (argc() + 2, 1),
        "array_from" => (argc(), 1),
        "call0" => (1, 1),
        "call1" => (2, 1),
        "call2" => (3, 1),
        "call3" => (4, 1),
        _ => return Err(format!("unsupported instruction {n} at {}", i.offset())),
    };
    Ok(e)
}
fn arguments_initializer(
    f: &FunctionImage,
    ins: &[Instruction<'_>],
    complex_parameters: bool,
) -> Result<(), String> {
    let initializers: Vec<_> = ins
        .iter()
        .enumerate()
        .filter(|(_, i)| i.opcode().name == "special_object" && i.operands()[0] <= 1)
        .collect();
    let mut tagged = None;
    for (slot, v) in f.locals.iter().enumerate() {
        if v.var_kind == VAR_KIND_ARGUMENTS && tagged.replace(slot).is_some() {
            return Err("materialized arguments tag requires one initializer".into());
        }
    }
    if initializers.is_empty() && tagged.is_some() {
        return Err("materialized arguments tag requires one initializer".into());
    }
    if initializers.is_empty() {
        return Ok(());
    }
    if initializers.len() != 1 {
        return Err("arguments object must be created exactly once".into());
    }
    let (at, op) = initializers[0];
    let mapped = op.operands()[0] == 1;
    // Only sloppy simple parameter lists alias native argument cells. A rest
    // parameter makes the list non-simple and therefore requires an unmapped
    // arguments object even when the function itself is sloppy.
    let requires_mapped = f.flags.simple_parameters && !f.strict;
    if !f.flags.arguments_allowed
        || !complex_parameters && f.defined_arg_count != f.arg_count
        || mapped != requires_mapped
    {
        return Err("unsupported arguments object function metadata".into());
    }
    if f.locals.len() != (f.arg_count + f.local_count) as usize {
        return Err("arguments object requires complete vardefs".into());
    }
    let mut names = BTreeSet::new();
    for v in &f.locals[..f.arg_count as usize] {
        if v.is_const
            || v.is_lexical
            || v.scope_level != 0
            || v.var_kind != 0
            || !v.scope_next.is_end()
            || mapped && !v.is_captured
            || !mapped && v.name.is_empty() && !complex_parameters
        {
            return Err("arguments parameter mapping metadata".into());
        }
        if !v.name.is_empty()
            && (!names.insert(&v.name) || v.name == "arguments".encode_utf16().collect::<Vec<_>>())
        {
            return Err("duplicate or shadowed arguments parameter name".into());
        }
    }
    let store = ins
        .get(at + 1)
        .ok_or("arguments initializer has no local store")?;
    let n = store.opcode().name;
    if !matches!(
        n,
        "put_loc" | "put_loc8" | "put_loc0" | "put_loc1" | "put_loc2" | "put_loc3"
    ) {
        return Err("arguments object needs immediate stable local store".into());
    }
    let slot = index(store);
    let v = f
        .locals
        .get(f.arg_count as usize + slot)
        .ok_or("arguments local slot")?;
    if v.name != "arguments".encode_utf16().collect::<Vec<_>>()
        || v.is_const
        || v.is_lexical
        || v.scope_level != 0
        || (v.var_kind != VAR_KIND_ARGUMENTS && (complex_parameters || v.var_kind != 0))
        || !v.is_captured
        || v.var_ref_idx >= f.var_ref_count
        || tagged.is_some_and(|tagged| tagged != f.arg_count as usize + slot)
    {
        return Err("arguments object binding metadata".into());
    }
    let arguments_name: Vec<_> = "arguments".encode_utf16().collect();
    let mut arguments_cells = 0;
    for (local, v) in f.locals[f.arg_count as usize..].iter().enumerate() {
        if v.scope_level != 0 || v.name != arguments_name {
            continue;
        }
        arguments_cells += 1;
        if arguments_cells > if complex_parameters { 2 } else { 1 } {
            return Err(
                "arguments function binding must be unique outside a split parameter environment"
                    .into(),
            );
        }
        // A qualified parameter-expression environment may have the stable
        // implicit arguments object and one distinct body-var arguments cell.
        // Both are real mutable captured locals; only the recorded initializer
        // above creates the arguments object, exactly once.
        if v.is_const
            || v.is_lexical
            || v.var_kind
                != if local == slot {
                    f.locals[f.arg_count as usize + slot].var_kind
                } else {
                    0
                }
            || !v.is_captured
            || v.var_ref_idx >= f.var_ref_count
        {
            return Err("split arguments body binding metadata".into());
        }
    }
    if arguments_cells == 0 {
        return Err("missing arguments function binding".into());
    }
    let mut prefix = 0;
    while prefix < at {
        if prefix == 0 && ins[prefix].opcode().name == "check_ctor" {
            prefix += 1;
            continue;
        }
        if ins[prefix].opcode().name != "undefined" || prefix + 1 >= at {
            return Err("arguments initialization must precede body effects".into());
        }
        let store = &ins[prefix + 1];
        if !matches!(
            store.opcode().name,
            "put_loc" | "put_loc8" | "put_loc0" | "put_loc1" | "put_loc2" | "put_loc3"
        ) {
            return Err("arguments initializer prefix".into());
        }
        let temp = f
            .locals
            .get(f.arg_count as usize + index(store))
            .ok_or("arguments prefix local index")?;
        if !temp.name.is_empty() || temp.is_lexical || temp.is_captured {
            return Err("arguments prefix may only initialize anonymous completion slots".into());
        }
        prefix += 2;
    }
    if ins.iter().any(|i| {
        i.branch_target()
            .is_some_and(|target| target <= store.offset() as i128)
    }) {
        return Err("arguments initializer can be reentered or bypassed".into());
    }
    Ok(())
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Handler {
    origin: usize,
    target: usize,
    depth: usize,
}
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(
    clippy::enum_variant_names,
    reason = "named after the loop statements they verify"
)]
enum IteratorKind {
    ForIn,
    ForOf,
    ForAwait,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct IteratorRecord {
    origin: usize,
    depth: usize,
    kind: IteratorKind,
}
impl IteratorRecord {
    fn width(self) -> usize {
        match self.kind {
            IteratorKind::ForIn => 1,
            IteratorKind::ForOf | IteratorKind::ForAwait => 3,
        }
    }
    fn end(self) -> usize {
        self.depth + self.width()
    }
}
// Completion control is part of the Rust proof, never a JS stack value or a
// numeric discriminator interpreted as a program counter. Source break and
// continue both arrive as a static Branch after emitter-owned scope cleanup.
// Duplicated finalizer bodies resume through these same closed continuations;
// their own Return/Throw/Branch terminates the prior source completion.
#[derive(Clone, Copy)]
enum Completion {
    Normal,
    Return,
    Throw,
    Branch {
        target: usize,
        conditional: bool,
    },
    EnterHandler {
        target: usize,
    },
    /// `with_*` lookup: the jump edge's stack differs from the fall-through by `delta`.
    With {
        target: usize,
        delta: isize,
    },
}
impl Completion {
    fn of(i: &Instruction<'_>, offsets: &BTreeMap<usize, usize>) -> Result<Self, String> {
        let n = i.opcode().name;
        if matches!(n, "return" | "return_undef" | "return_async") {
            return Ok(Self::Return);
        }
        if matches!(n, "throw" | "throw_error") || n == "strict_op" && i.operands()[4] == 5 {
            return Ok(Self::Throw);
        }
        if let Some(target) = i.branch_target() {
            let target = usize::try_from(target).map_err(|_| "completion target range")?;
            let target = *offsets.get(&target).ok_or("completion target boundary")?;
            return match n {
                "catch" => Ok(Self::EnterHandler { target }),
                "goto" | "goto8" | "goto16" => Ok(Self::Branch {
                    target,
                    conditional: false,
                }),
                "if_false" | "if_true" | "if_false8" | "if_true8" => Ok(Self::Branch {
                    target,
                    conditional: true,
                }),
                "with_get_var" | "with_delete_var" => Ok(Self::With { target, delta: 1 }),
                "with_get_ref" | "with_make_ref" => Ok(Self::With { target, delta: 2 }),
                "with_put_var" => Ok(Self::With { target, delta: -1 }),
                _ => Err("unqualified dynamic/iterator completion control".into()),
            };
        }
        Ok(Self::Normal)
    }
}
#[derive(Clone, Copy)]
enum ExceptionSink {
    CatchBinding(usize),
    // A saved thrown JS value has no native scope/capture authority. Its slot is
    // anonymous and unshared so direct eval/closures cannot name or alias it.
    CompletionTemporary,
}
fn exception_sink(v: &VarDef, slot: usize) -> Result<ExceptionSink, String> {
    if v.var_kind == 3 && v.is_captured && !v.is_lexical && !v.is_const && v.scope_level > 0 {
        return Ok(ExceptionSink::CatchBinding(slot));
    }
    if v.name.is_empty()
        && v.var_kind == 0
        && v.scope_level == 0
        && v.scope_next.is_end()
        && !v.is_captured
        && !v.is_const
        && !v.is_lexical
    {
        return Ok(ExceptionSink::CompletionTemporary);
    }
    Err("exception value requires a catch binding or anonymous completion temporary".into())
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct ApplyArray {
    target: usize,
    depth: usize,
}
#[derive(Clone)]
struct FrameState {
    stack: usize,
    locals: Vec<u8>,
    activity: Vec<u8>,
    // Native catch tags are never JS values. Their sparse slot identities must
    // agree at joins, and no ordinary operand may consume or rearrange them.
    handlers: Vec<Handler>,
    // ForOf's three slots are iterator,next,private catch-offset zero. Only a
    // successful native start creates this authority; ordinary JS values and
    // OP_catch target zero cannot manufacture it. ForIn's opaque enum is also
    // kept intact until its matching next/drop operation.
    iterators: Vec<IteratorRecord>,
    // apply magic1 falls back to a normal call for null/undefined arglists.
    // Constructor-result authority therefore requires a proven private array
    // value retained through a straight sequence of anonymous scratch clears.
    apply_array: Option<ApplyArray>,
    pending_exception: Option<usize>,
    // A generator frame must reach its single initial_yield before it can
    // suspend or complete: a frame that completes during the call would later
    // be resumed after it has finished.
    started: bool,
}
struct StateBudget {
    remaining: usize,
}
impl StateBudget {
    fn new(limits: &ImageLimits) -> Self {
        Self {
            remaining: limits.max_state_cells,
        }
    }
    fn reserve(&mut self, cells: usize) -> Result<(), String> {
        self.remaining = self
            .remaining
            .checked_sub(cells)
            .ok_or("capacity: exception verification state budget")?;
        Ok(())
    }
}
fn scoped(v: &VarDef) -> bool {
    v.is_lexical && v.scope_level > 0 || v.var_kind == 3
}
// An omitted case conservatively gets an exceptional successor. In particular,
// property/coercion/call/allocation/check/debug operations are never silently
// classified as nonthrowing. These exemptions perform direct bounded moves or
// primitive pushes without a native exception branch.
fn may_throw(n: &str) -> bool {
    if n.starts_with("get_loc") && n != "get_loc_check"
        || n.starts_with("put_loc") && !n.contains("check")
        || n.starts_with("set_loc") && !n.contains("check")
        || n.starts_with("get_arg")
        || n.starts_with("put_arg")
        || n.starts_with("set_arg")
        || n.contains("var_ref")
            && !n.contains("check")
            && (n.starts_with("get_") || n.starts_with("put_") || n.starts_with("set_"))
    {
        return false;
    }
    !matches!(
        n,
        "catch"
            | "close_loc"
            | "drop"
            | "nop"
            | "return"
            | "return_undef"
            | "return_async"
            | "push_i32"
            | "push_i16"
            | "push_i8"
            | "push_minus1"
            | "push_0"
            | "push_1"
            | "push_2"
            | "push_3"
            | "push_4"
            | "push_5"
            | "push_6"
            | "push_7"
            | "push_const"
            | "push_const8"
            | "undefined"
            | "null"
            | "push_false"
            | "push_true"
            | "dup"
            | "nip"
            | "nip1"
            | "dup1"
            | "dup2"
            | "dup3"
            | "insert2"
            | "insert3"
            | "insert4"
            | "swap"
            | "swap2"
            | "perm3"
            | "perm4"
            | "perm5"
            | "rot3l"
            | "rot3r"
            | "rot4l"
            | "rot5l"
    )
}
// Capture tables are conservatively over-allocated by the emitter. Only an
// actual read/write (including forwarding to a used descendant capture) grants
// a child access to a catch binding, so unused entries need not activate it.
fn capture_is_used(
    f: &FunctionImage,
    wanted: usize,
    work: &mut limits::Work,
) -> Result<bool, String> {
    let mut at = 0;
    while at < f.code.len() {
        work.charge(1)?;
        let op = f.code[at]
            .checked_sub(1)
            .and_then(|id| crate::production::FINAL_OPCODES.get(id as usize))
            .ok_or("capture-use opcode")?;
        let end = at
            .checked_add(op.width as usize)
            .ok_or("capture-use width")?;
        let operands = f.code.get(at + 1..end).ok_or("capture-use operands")?;
        let n = op.name;
        if n.contains("var_ref")
            && (n.starts_with("get_") || n.starts_with("put_") || n.starts_with("set_"))
        {
            let index = if operands.len() >= 2 {
                usize::from(u16::from_le_bytes([operands[0], operands[1]]))
            } else if let Some(index) = operands.first() {
                usize::from(*index)
            } else {
                usize::from(*n.as_bytes().last().ok_or("capture-use instruction")? - b'0')
            };
            if index == wanted {
                return Ok(true);
            }
        }
        if matches!(n, "fclosure" | "fclosure8" | "push_const" | "push_const8") {
            let index = if operands.len() == 1 {
                usize::from(operands[0])
            } else {
                u32::from_le_bytes(
                    operands
                        .get(..4)
                        .ok_or("closure operand")?
                        .try_into()
                        .unwrap(),
                ) as usize
            };
            let child = match f.constants.get(index) {
                Some(Constant::Function(child)) => Some(child),
                Some(_) if n.starts_with("push_const") => None,
                _ => return Err("closure constant kind".into()),
            };
            let Some(child) = child else {
                at = end;
                continue;
            };
            for (index, capture) in child.captures.iter().enumerate() {
                work.charge(1)?;
                if matches!(capture.closure_type, 2 | 3)
                    && capture.var_idx as usize == wanted
                    && capture_is_used(child, index, work)?
                {
                    return Ok(true);
                }
            }
        }
        at = end;
    }
    Ok(false)
}
fn merge_state(
    next: usize,
    incoming: &FrameState,
    states: &mut [Option<FrameState>],
    queue: &mut VecDeque<usize>,
    queued: &mut [bool],
    budget: &mut StateBudget,
    work: &mut limits::Work,
) -> Result<(), String> {
    work.charge(
        1 + 2 * incoming.locals.len()
            + 3 * incoming.handlers.len()
            + 3 * incoming.iterators.len()
            + 2 * usize::from(incoming.apply_array.is_some()),
    )?;
    let changed = match &mut states[next] {
        None => {
            budget.reserve(
                3 * incoming.handlers.len()
                    + 3 * incoming.iterators.len()
                    + 2 * usize::from(incoming.apply_array.is_some()),
            )?;
            states[next] = Some(incoming.clone());
            true
        }
        Some(old) => {
            if old.stack != incoming.stack
                || old.handlers != incoming.handlers
                || old.iterators != incoming.iterators
                || old.apply_array != incoming.apply_array
                || old.pending_exception != incoming.pending_exception
                || old.started != incoming.started
            {
                return Err("stack/handler mismatch at join".into());
            }
            let mut changed = false;
            for (a, b) in old.locals.iter_mut().zip(&incoming.locals) {
                let value = (*a).min(*b);
                changed |= value != *a;
                *a = value;
            }
            for (a, b) in old.activity.iter_mut().zip(&incoming.activity) {
                let value = *a | *b;
                changed |= value != *a;
                *a = value;
            }
            changed
        }
    };
    if changed && !queued[next] {
        queued[next] = true;
        queue.push_back(next);
    }
    Ok(())
}
// Marker scope is a Rust emitter/verifier protocol. Native close_loc detaches
// references without changing var_buf; re-entry requires a sentinel operation.
// A real parameter-expression scope has a native -2 tail, not a magic marker
// number. Only this explicit split profile may use anonymous raw argv and a
// shorter function length. Parameter source cells are distinct captured TDZ
// locals; raw argv is neither named nor shared with arguments/closures.
fn parameter_head(f: &FunctionImage, work: &mut limits::Work) -> Result<Option<usize>, String> {
    work.charge(f.locals.len())?;
    if !f
        .locals
        .iter()
        .any(|v| v.scope_next == ScopeLink::Parameters)
    {
        return Ok(None);
    }
    if f.flags.simple_parameters || f.locals.len() != (f.arg_count + f.local_count) as usize {
        return Err("parameter scope requires a complete non-simple function".into());
    }
    for v in &f.locals[..f.arg_count as usize] {
        work.charge(1)?;
        if !v.name.is_empty()
            || v.is_captured
            || v.is_const
            || v.is_lexical
            || v.var_kind != 0
            || v.scope_level != 0
            || !v.scope_next.is_end()
        {
            return Err(
                "parameter expression argv must be anonymous immutable unshared slots".into(),
            );
        }
    }
    let mut count = 0;
    let mut head = None;
    let mut anonymous_anchor = None;
    for (slot, v) in f.locals[f.arg_count as usize..].iter().enumerate() {
        work.charge(1)?;
        if v.scope_level == 1 {
            if !v.is_lexical || !v.is_captured || v.is_const || v.var_kind != 0 {
                return Err("parameter cells require captured mutable lexical metadata".into());
            }
            if v.name.is_empty() && anonymous_anchor.replace(slot).is_some() {
                return Err("parameter scope has multiple anonymous anchors".into());
            }
            match v.scope_next {
                ScopeLink::Parameters => {}
                ScopeLink::Local(previous)
                    if (previous as usize) < slot
                        && f.locals[f.arg_count as usize + previous as usize].scope_level == 1 => {}
                _ => {
                    return Err("parameter scope chain must have its genuine parameter tail".into());
                }
            }
            count += 1;
            head = Some(slot);
        } else if v.scope_next == ScopeLink::Parameters {
            return Err("only parameter cells may terminate the parameter scope".into());
        }
    }
    let head = head.ok_or("empty parameter-expression scope")?;
    if anonymous_anchor.is_some() && count != 1 {
        return Err(
            "anonymous parameter anchor is only valid for a zero-binding pattern list".into(),
        );
    }
    let mut next = ScopeLink::Local(head as u32);
    let mut visited = 0;
    while let ScopeLink::Local(slot) = next {
        work.charge(1)?;
        visited += 1;
        next = f.locals[f.arg_count as usize + slot as usize].scope_next;
    }
    if next != ScopeLink::Parameters || visited != count {
        return Err("parameter chain does not cover every parameter cell".into());
    }
    Ok(Some(head))
}
fn new_target_initializer(
    f: &FunctionImage,
    ins: &[Instruction<'_>],
    work: &mut limits::Work,
) -> Result<(), String> {
    let mut seen = [false; 2];
    for (at, op) in ins.iter().enumerate() {
        work.charge(1)?;
        if op.opcode().name != "special_object" || !matches!(op.operands()[0], 3 | 4) {
            continue;
        }
        let variant = op.operands()[0];
        let context = (variant - 3) as usize;
        let permission = if variant == 3 {
            f.flags.new_target
        } else {
            f.flags.need_home_object
        };
        let name = if variant == 3 {
            "new.target"
        } else {
            "<home_object>"
        };
        if seen[context] || !permission {
            return Err("new.target requires one permitted ordinary owner initializer".into());
        }
        seen[context] = true;
        let store = ins.get(at + 1).ok_or("native context initializer store")?;
        if !matches!(
            store.opcode().name,
            "put_loc" | "put_loc8" | "put_loc0" | "put_loc1" | "put_loc2" | "put_loc3"
        ) {
            return Err("new.target needs an immediate stable local store".into());
        }
        let v = f
            .locals
            .get(f.arg_count as usize + index(store))
            .ok_or("new.target owner vardef")?;
        if v.name != name.encode_utf16().collect::<Vec<_>>()
            || !v.is_captured
            || v.is_const
            || v.is_lexical
            || v.var_kind != 0
            || v.scope_level != 0
            || v.var_ref_idx >= f.var_ref_count
        {
            return Err("new.target owner binding metadata".into());
        }
        work.charge(ins.len())?;
        if ins.iter().any(|i| {
            i.branch_target()
                .is_some_and(|target| target <= store.offset() as i128)
        }) {
            return Err("new.target owner initializer can be bypassed or reentered".into());
        }
    }
    Ok(())
}
fn body_anchor(
    f: &FunctionImage,
    parameter_head: Option<usize>,
    ins: &[Instruction<'_>],
    work: &mut limits::Work,
) -> Result<Option<usize>, String> {
    let mut anchor = None;
    if f.locals.iter().take(f.arg_count as usize).any(|v| {
        matches!(
            v.var_kind,
            VAR_KIND_BODY_ANCHOR | VAR_KIND_BODY_ARGUMENTS_ANCHOR
        )
    }) {
        return Err("body anchor cannot occupy raw argv".into());
    }
    let definitions = f.locals.get(f.arg_count as usize..).unwrap_or(&[]);
    for (slot, v) in definitions.iter().enumerate() {
        work.charge(1)?;
        if !matches!(
            v.var_kind,
            VAR_KIND_BODY_ANCHOR | VAR_KIND_BODY_ARGUMENTS_ANCHOR
        ) {
            continue;
        }
        if anchor.replace(slot).is_some()
            || parameter_head.is_none()
            || !v.name.is_empty()
            || !v.is_lexical
            || !v.is_captured
            || v.is_const
            || v.scope_level != 2
        {
            return Err("invalid split-body scope anchor metadata".into());
        }
        let body_arguments = f.locals[f.arg_count as usize..].iter().any(|body| {
            body.scope_level == 0
                && body.var_kind == 0
                && !body.is_lexical
                && body.name == "arguments".encode_utf16().collect::<Vec<_>>()
        });
        if (v.var_kind == VAR_KIND_BODY_ARGUMENTS_ANCHOR) != body_arguments {
            return Err("split-body arguments shadow proof mismatch".into());
        }
        let mut next = v.scope_next;
        let mut saw_parameter = false;
        while let ScopeLink::Local(index) = next {
            work.charge(1)?;
            saw_parameter |= Some(index as usize) == parameter_head;
            next = f.locals[f.arg_count as usize + index as usize].scope_next;
        }
        if next != ScopeLink::Parameters || !saw_parameter {
            return Err("body scope anchor must retain its parameter environment".into());
        }
        let initializers = ins
            .iter()
            .filter(|i| i.opcode().name == "set_loc_uninitialized" && index(i) == slot)
            .count();
        work.charge(ins.len())?;
        if initializers != 1 {
            return Err("body scope anchor requires one activation".into());
        }
    }
    if parameter_head.is_some()
        && ins.iter().any(|i| i.opcode().name == "debug_stmt")
        && anchor.is_none()
    {
        return Err("marked split function requires a body scope anchor".into());
    }
    Ok(anchor)
}

fn marker_chain(
    f: &FunctionImage,
    i: &Instruction<'_>,
    work: &mut limits::Work,
    parameter_head: Option<usize>,
) -> Result<Vec<usize>, String> {
    let bits = u32::from_le_bytes(i.operands().try_into().unwrap());
    let head = bits & 0xffff;
    if head == 0xffff {
        return Err("unqualified parameter/maximum marker head".into());
    }
    if f.locals.len() != (f.arg_count + f.local_count) as usize {
        return Err("marked functions require complete vardefs".into());
    }
    let mut next = if head == 0 {
        ScopeLink::End
    } else {
        ScopeLink::Local(head - 1)
    };
    let mut chain = Vec::new();
    let mut level = u32::MAX;
    loop {
        match next {
            ScopeLink::End => break,
            ScopeLink::Parameters => {
                work.charge(chain.len())?;
                if !parameter_head.is_some_and(|head| chain.contains(&head)) {
                    return Err(
                        "parameter marker requires the qualified complete parameter chain".into(),
                    );
                }
                break;
            }
            ScopeLink::Local(n) => {
                work.charge(1)?;
                if n >= f.local_count {
                    return Err("marker scope head/link index".into());
                }
                let v = &f.locals[(f.arg_count + n) as usize];
                if !scoped(v) || v.scope_level == 0 {
                    return Err("marker chain contains nonlexical slot".into());
                }
                if v.scope_level > level {
                    return Err("marker scope chain reverses lexical nesting".into());
                }
                level = v.scope_level;

                if chain.len() >= f.local_count as usize {
                    return Err("marker scope cycle".into());
                }
                chain.push(n as usize);
                next = v.scope_next;
            }
        }
    }
    Ok(chain)
}
fn index(i: &Instruction<'_>) -> usize {
    let n = i.opcode().name;
    let b = i.operands();
    if b.len() >= 2 {
        usize::from(u16::from_le_bytes([b[0], b[1]]))
    } else if !b.is_empty() {
        usize::from(b[0])
    } else {
        usize::from(n.as_bytes()[n.len() - 1] - b'0')
    }
}
fn atom_format(f: OperandFormat) -> bool {
    matches!(
        f,
        OperandFormat::Atom
            | OperandFormat::AtomU8
            | OperandFormat::AtomU16
            | OperandFormat::AtomLabelU8
            | OperandFormat::AtomLabelU16
    )
}
fn string_budget(s: &[u16]) -> Result<(), String> {
    if s.len() > 8 * 1024 * 1024 {
        Err("capacity: string budget".into())
    } else {
        Ok(())
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum FunctionRole {
    Ordinary,
    HomeBound,
    ClassConstructor,
    LexicalSuper,
}
fn derived_this_slot(f: &FunctionImage) -> Result<Option<usize>, String> {
    if !f.flags.derived_constructor {
        return Ok(None);
    }
    let name: Vec<_> = "this".encode_utf16().collect();
    let mut found = None;
    for (slot, v) in f
        .locals
        .get(f.arg_count as usize..)
        .ok_or("derived constructor vardefs")?
        .iter()
        .enumerate()
    {
        if v.name != name {
            continue;
        }
        if found.is_some()
            || !v.is_captured
            || !v.is_lexical
            || v.is_const
            || v.scope_level != 0
            || v.var_kind != 0
            || v.var_ref_idx >= f.var_ref_count
        {
            return Err(
                "derived this requires one captured lexical function-scope TDZ cell".into(),
            );
        }
        found = Some(slot);
    }
    found
        .map(Some)
        .ok_or_else(|| "derived constructor has no this TDZ cell".into())
}
fn plain_local_put(n: &str) -> bool {
    matches!(
        n,
        "put_loc" | "put_loc8" | "put_loc0" | "put_loc1" | "put_loc2" | "put_loc3"
    )
}
fn plain_local_get(n: &str) -> bool {
    matches!(
        n,
        "get_loc" | "get_loc8" | "get_loc0" | "get_loc1" | "get_loc2" | "get_loc3"
    )
}
fn anonymous_scratch(v: &VarDef) -> bool {
    v.name.is_empty()
        && !v.is_captured
        && !v.is_const
        && !v.is_lexical
        && v.var_kind == 0
        && v.scope_level == 0
        && v.scope_next.is_end()
}
fn zero_array(i: &Instruction<'_>) -> bool {
    i.opcode().name == "array_from" && i.operands() == [0, 0]
}
/// What [`constructor_apply_arrays`] proves: keyed by the index of the instruction that loads
/// a constructor `apply`'s argument array, the index of that `apply` and the local slot the
/// array is loaded from; and the set of those slots.
type ApplyArrays = (BTreeMap<usize, (usize, usize)>, BTreeSet<usize>);
// No runtime PC or JS type assertion is trusted here. A constructor apply must
// receive an actual freshly-created private array, not the helper's nullable
// normal-call shortcut. Local fact 3 denotes that proven array; 2 remains an
// arbitrary initialized JS value. Ordinary joins conservatively take min.
fn constructor_apply_arrays(
    f: &FunctionImage,
    ins: &[Instruction<'_>],
    budget: &mut StateBudget,
    work: &mut limits::Work,
) -> Result<ApplyArrays, String> {
    let mut loads = BTreeMap::new();
    let mut slots = BTreeSet::new();
    for (at, i) in ins.iter().enumerate() {
        work.charge(1)?;
        if i.opcode().name != "apply" || u16::from_le_bytes(i.operands().try_into().unwrap()) != 1 {
            continue;
        }
        let mut load = at;
        while load >= 2
            && plain_local_put(ins[load - 1].opcode().name)
            && ins[load - 2].opcode().name == "undefined"
        {
            work.charge(1)?;
            let slot = index(&ins[load - 1]);
            if !f
                .locals
                .get(f.arg_count as usize + slot)
                .is_some_and(anonymous_scratch)
            {
                return Err("constructor apply clears must use unshared anonymous scratch".into());
            }
            load -= 2;
        }
        let load = load.checked_sub(1).ok_or("constructor apply array load")?;
        if !plain_local_get(ins[load].opcode().name) {
            return Err("constructor apply requires a proven private argument-array load".into());
        }
        let slot = index(&ins[load]);
        if !f
            .locals
            .get(f.arg_count as usize + slot)
            .is_some_and(anonymous_scratch)
        {
            return Err("constructor apply array must have private anonymous storage".into());
        }
        work.charge(ins.len())?;
        if ins.iter().any(|branch| {
            branch.branch_target().is_some_and(|target| {
                target > ins[load].offset() as i128 && target <= i.offset() as i128
            })
        }) {
            return Err(
                "control entry cannot counterfeit a retained constructor argument array".into(),
            );
        }
        budget.reserve(3)?;
        if loads.insert(load, (at, slot)).is_some() {
            return Err("constructor argument load has conflicting continuations".into());
        }
        if !slots.contains(&slot) {
            budget.reserve(1)?;
            slots.insert(slot);
        }
    }
    // A plain local fact can become known-array only through this direct native
    // producer/store pair. Neither a jump nor a catch entry may skip creation.
    for (at, store) in ins.iter().enumerate() {
        work.charge(1)?;
        if !plain_local_put(store.opcode().name) || !slots.contains(&index(store)) {
            continue;
        }
        if at > 0 && zero_array(&ins[at - 1]) {
            work.charge(ins.len())?;
            if ins
                .iter()
                .any(|branch| branch.branch_target() == Some(store.offset() as i128))
            {
                return Err("control entry cannot bypass private argument-array creation".into());
            }
        }
    }
    Ok((loads, slots))
}
fn constructor_result_prefix(
    ins: &[Instruction<'_>],
    at: usize,
    work: &mut limits::Work,
) -> Result<(), String> {
    let previous = at.checked_sub(1).ok_or("missing constructor result")?;
    let result = if ins[previous].opcode().name == "dup" {
        previous
            .checked_sub(1)
            .ok_or("missing duplicated constructor result")?
    } else {
        previous
    };
    if !matches!(ins[result].opcode().name, "call_constructor" | "init_ctor")
        && !(ins[result].opcode().name == "apply"
            && u16::from_le_bytes(ins[result].operands().try_into().unwrap()) == 1)
    {
        return Err("this initialization requires an adjacent constructor result".into());
    }
    work.charge(ins.len())?;
    if ins.iter().any(|branch| {
        branch.branch_target().is_some_and(|target| {
            target == ins[at].offset() as i128
                || previous != result && target == ins[previous].offset() as i128
        })
    }) {
        return Err("control entry cannot forge a constructor result for this".into());
    }
    Ok(())
}
fn derived_return_profile(
    ins: &[Instruction<'_>],
    slot: usize,
    work: &mut limits::Work,
) -> Result<(), String> {
    let mut sentinel = false;
    for (at, i) in ins.iter().enumerate() {
        work.charge(1)?;
        let n = i.opcode().name;
        if n == "return_undef" {
            return Err("derived return requires an object or checked this".into());
        }
        if n == "push_this" {
            return Err("derived this cannot use the incoming receiver".into());
        }
        if !sentinel
            && !matches!(
                n,
                "check_ctor"
                    | "special_object"
                    | "undefined"
                    | "set_loc_uninitialized"
                    | "put_loc"
                    | "put_loc8"
                    | "put_loc0"
                    | "put_loc1"
                    | "put_loc2"
                    | "put_loc3"
            )
        {
            return Err("derived this TDZ must precede parameter/body effects".into());
        }
        if n != "get_loc0_loc1" && n.contains("loc") && index(i) == slot {
            match n {
                "set_loc_uninitialized" => {
                    if sentinel {
                        return Err("derived this TDZ cannot be reset".into());
                    }
                    sentinel = true;
                }
                "get_loc_check" => {}
                "put_loc_check_init" => {
                    constructor_result_prefix(ins, at, work)?;
                }
                _ => {
                    return Err(
                        "derived this requires checked reads and one-time checked initialization"
                            .into(),
                    );
                }
            }
        }
        if n == "get_loc0_loc1" && slot < 2 {
            return Err("paired loads cannot bypass derived this TDZ".into());
        }
        if n == "check_ctor_return" {
            let branch = ins.get(at + 1).ok_or("derived return branch")?;
            if !matches!(branch.opcode().name, "if_false" | "if_false8") {
                return Err("derived return check needs its immediate object branch".into());
            }
            let target = branch
                .branch_target()
                .ok_or("derived object return target")?;
            work.charge(ins.len())?;
            let target = ins
                .iter()
                .position(|i| i.offset() as i128 == target)
                .ok_or("derived object return boundary")?;
            if ins[target].opcode().name != "return" || target <= at + 1 {
                return Err("derived object branch must return its checked object".into());
            }
            work.charge(ins.len())?;
            if ins.iter().enumerate().any(|(index, other)| {
                other.branch_target().is_some_and(|target_pc| {
                    target_pc == branch.offset() as i128
                        || target_pc == ins[target].offset() as i128 && index != at + 1
                })
            }) {
                return Err(
                    "derived return continuation cannot be forged by another control edge".into(),
                );
            }
            if target == 0
                || !matches!(
                    ins[target - 1].opcode().name,
                    "return" | "throw" | "throw_error" | "goto" | "goto8" | "goto16"
                )
            {
                return Err("ordinary fallthrough cannot enter a checked object return".into());
            }
        }
        if n == "return" {
            let checked_this =
                at.checked_sub(1)
                    .and_then(|at| ins.get(at))
                    .is_some_and(|previous| {
                        previous.opcode().name == "get_loc_check" && index(previous) == slot
                    });
            work.charge(ins.len())?;
            let checked_object = ins.iter().enumerate().any(|(check_at, check)| {
                check.opcode().name == "check_ctor_return"
                    && ins.get(check_at + 1).is_some_and(|branch| {
                        matches!(branch.opcode().name, "if_false" | "if_false8")
                            && branch.branch_target() == Some(i.offset() as i128)
                    })
            });
            if !checked_this && !checked_object {
                return Err("derived return has no object/initialized-this proof".into());
            }
            if checked_this {
                work.charge(ins.len())?;
                if ins
                    .iter()
                    .any(|branch| branch.branch_target() == Some(i.offset() as i128))
                {
                    return Err("return entry cannot bypass a checked derived this load".into());
                }
            }
        }
    }
    if !sentinel {
        return Err("derived this TDZ must be installed before parameter/body execution".into());
    }
    Ok(())
}
fn constant_index(i: &Instruction<'_>) -> Result<usize, String> {
    if i.operands().len() == 1 {
        return Ok(usize::from(i.operands()[0]));
    }
    Ok(u32::from_le_bytes(
        i.operands()
            .get(..4)
            .ok_or("constant operand")?
            .try_into()
            .unwrap(),
    ) as usize)
}
// Native define_class dereferences its top value as JSFunctionBytecode without
// a tag guard. A raw constant must therefore be an indivisible adjacent pair:
// no opcode or normal/exception entry can observe or substitute that value.
fn class_constant_roles(
    f: &FunctionImage,
    ins: &[Instruction<'_>],
    budget: &mut StateBudget,
    work: &mut limits::Work,
) -> Result<BTreeMap<usize, FunctionRole>, String> {
    let mut roles = BTreeMap::new();
    for (at, i) in ins.iter().enumerate() {
        work.charge(1)?;
        let n = i.opcode().name;
        if matches!(n, "define_class" | "define_class_computed") {
            let previous = at
                .checked_sub(1)
                .and_then(|previous| ins.get(previous))
                .ok_or("class needs a raw constructor constant")?;
            if !matches!(previous.opcode().name, "push_const" | "push_const8") {
                return Err("class constructor must be an adjacent raw function constant".into());
            }
            let child = match f.constants.get(constant_index(previous)?) {
                Some(Constant::Function(child)) => child,
                _ => return Err("class constructor constant kind".into()),
            };
            if !child.strict
                || child.flags.kind != 0
                || child.flags.has_prototype
                || !child.flags.new_target
                || child.flags.derived_constructor != (i.operands()[4] & 1 != 0)
            {
                return Err("class constructor flags/heritage mismatch".into());
            }
            work.charge(ins.len())?;
            if ins
                .iter()
                .any(|branch| branch.branch_target() == Some(i.offset() as i128))
            {
                return Err("control entry cannot bypass a raw class constructor constant".into());
            }
        }
        if !matches!(n, "push_const" | "push_const8" | "fclosure" | "fclosure8") {
            continue;
        }
        let index = constant_index(i)?;
        let Some(Constant::Function(child)) = f.constants.get(index) else {
            continue;
        };
        let next = ins.get(at + 1);
        let role = if n.starts_with("push_const") {
            if !next.is_some_and(|next| {
                matches!(next.opcode().name, "define_class" | "define_class_computed")
            }) {
                return Err("raw function bytecode cannot escape its class-definition pair".into());
            }
            FunctionRole::ClassConstructor
        } else if next.is_some_and(|next| {
            matches!(
                next.opcode().name,
                "define_method" | "define_method_computed" | "set_home_object"
            )
        }) {
            if child.flags.has_prototype {
                return Err("bound methods must be nonconstructible".into());
            }
            FunctionRole::HomeBound
        } else if (child.flags.super_allowed || child.flags.super_call)
            && !child.flags.has_prototype
            && !child.flags.need_home_object
            && !child.flags.derived_constructor
            && f.flags.super_allowed
            && (!child.flags.super_call || f.flags.super_call)
        {
            FunctionRole::LexicalSuper
        } else {
            if child.flags.need_home_object
                || child.flags.super_allowed
                || child.flags.super_call
                || child.flags.derived_constructor
            {
                return Err(
                    "home/constructor bytecode requires a qualified installation context".into(),
                );
            }
            FunctionRole::Ordinary
        };
        match roles.get(&index) {
            Some(previous) if *previous != role => {
                return Err("function constant has conflicting installation roles".into());
            }
            Some(_) => {}
            None => {
                budget.reserve(1)?;
                roles.insert(index, role);
            }
        }
    }
    Ok(roles)
}
fn class_permissions(
    f: &FunctionImage,
    ins: &[Instruction<'_>],
    role: FunctionRole,
    work: &mut limits::Work,
) -> Result<(), String> {
    if f.flags.derived_constructor && role != FunctionRole::ClassConstructor
        || f.flags.super_call
            && !(f.flags.derived_constructor || role == FunctionRole::LexicalSuper)
        || f.flags.need_home_object
            && !matches!(
                role,
                FunctionRole::HomeBound | FunctionRole::ClassConstructor
            )
    {
        return Err("class/context permission is not backed by its installation role".into());
    }
    if (f.flags.super_allowed || f.flags.need_home_object) && role == FunctionRole::Ordinary {
        return Err("home/super flags require a qualified bound function".into());
    }
    if role == FunctionRole::LexicalSuper
        && ins.iter().any(|i| {
            i.opcode().name == "special_object" && i.operands()[0] != 6
                || i.opcode().name == "push_this"
        })
    {
        return Err("lexical super functions must use captured owner context".into());
    }
    if role == FunctionRole::ClassConstructor {
        if f.flags.derived_constructor && (!f.flags.super_call || !f.flags.super_allowed) {
            return Err("derived constructor must retain its native super permissions".into());
        }
        if !f.strict
            || f.flags.has_prototype
            || !f.flags.new_target
            || f.flags.kind != 0
            || f.locals.len() != (f.arg_count + f.local_count) as usize
        {
            return Err("class constructor function metadata".into());
        }
        for argument in &f.locals[..f.arg_count as usize] {
            work.charge(1)?;
            if ["this", "new.target", "<home_object>", "this.active_func"]
                .iter()
                .any(|name| argument.name == name.encode_utf16().collect::<Vec<_>>())
            {
                return Err(
                    "constructor argument cannot impersonate a native owner binding".into(),
                );
            }
        }
        if ins.first().is_none_or(|i| i.opcode().name != "check_ctor") {
            return Err("class constructor must start with its native call guard".into());
        }
        if let Some(slot) = derived_this_slot(f)? {
            derived_return_profile(ins, slot, work)?;
        }
    } else if ins.iter().any(|i| i.opcode().name == "check_ctor") {
        return Err("constructor guard requires a class-constructor installation role".into());
    }
    Ok(())
}
#[derive(Clone, Copy)]
enum RootContext<'a> {
    Ordinary,
    Module,
    Eval(&'a super::eval::VerifiedEnvLayout),
}
pub(super) fn verify_eval_with_limits(
    f: &FunctionImage,
    env: &super::eval::VerifiedEnvLayout,
    l: &ImageLimits,
) -> Result<usize, String> {
    if env.parameter_scope()
        || env
            .layout()
            .locals
            .iter()
            .any(|v| v.scope_next == ScopeLink::Parameters)
    {
        return Err("parameter-environment eval is not implemented".into());
    }
    if f.arg_count != 0
        || f.defined_arg_count != 0
        || f.flags.has_prototype
        || env.layout().strict && !f.strict
    {
        return Err("eval entrypoint/strictness mismatch".into());
    }
    let caller = &env.layout().flags;
    if f.flags.new_target && !caller.new_target
        || f.flags.arguments_allowed && !caller.arguments_allowed
        || f.flags.super_allowed
        || f.flags.super_call
        || f.flags.derived_constructor
        || f.flags.need_home_object
    {
        return Err("eval inherited permission mismatch".into());
    }
    let mut work = limits::preflight(f, None, l)?;
    let mut budget = StateBudget::new(l);
    verify_function(
        f,
        None,
        RootContext::Eval(env),
        l,
        &mut work,
        &mut budget,
        FunctionRole::Ordinary,
    )?;
    Ok(l.max_work - work.remaining())
}
pub(super) fn verify_with_limits(f: &FunctionImage, l: &ImageLimits) -> Result<usize, String> {
    let mut work = limits::preflight(f, None, l)?;
    let mut budget = StateBudget::new(l);
    verify_function(
        f,
        None,
        RootContext::Ordinary,
        l,
        &mut work,
        &mut budget,
        FunctionRole::Ordinary,
    )?;
    Ok(l.max_work - work.remaining())
}
fn verify_function(
    f: &FunctionImage,
    parent: Option<&FunctionImage>,
    root: RootContext<'_>,
    l: &ImageLimits,
    work: &mut limits::Work,
    budget: &mut StateBudget,
    role: FunctionRole,
) -> Result<(), String> {
    let module_root = matches!(root, RootContext::Module);
    let eval_root = if let RootContext::Eval(env) = root {
        Some(env)
    } else {
        None
    };
    if f.arg_count > 65535
        || f.local_count > 65535
        || f.defined_arg_count > f.arg_count
        || f.stack_size > 65535
        || f.var_ref_count > 65535
        || f.constants.len() > 65535
        || f.captures.len() > 65535
        || f.atoms.len() > 65535
    {
        return Err("capacity: function metadata budget".into());
    }
    let kind_allowed = if module_root {
        f.flags.kind == 2
    } else if eval_root.is_some() {
        f.flags.kind == 0
    } else {
        f.flags.kind <= 3
    };
    if !kind_allowed {
        return Err("unsupported suspension function kind".into());
    }
    if f.filename.len() >= 16384 || f.filename.contains('\0') {
        return Err("filename native bound".into());
    }
    if f.function_name.len() > 16 * 1024 * 1024 {
        return Err("capacity: function name budget".into());
    }
    if !f.locals.is_empty() && f.locals.len() != (f.arg_count + f.local_count) as usize {
        return Err("vardef count mismatch".into());
    }
    let mut refs = BTreeSet::new();
    for (at, v) in f.locals.iter().enumerate() {
        string_budget(&v.name)?;
        if (5..=9).contains(&v.var_kind)
            && (!v.is_const || !v.is_lexical || !v.is_captured || v.scope_level == 0)
        {
            return Err("private class binding requires a captured lexical constant".into());
        }
        if v.var_kind == 3
            && (at < f.arg_count as usize
                || v.is_const
                || v.is_lexical
                || v.scope_level == 0
                || !v.is_captured)
        {
            return Err("catch binding requires captured scoped nonlexical vardef".into());
        }
        if v.var_kind > 15
            || v.scope_level > 65535
            || v.scope_next
                .local_index()
                .is_some_and(|n| n >= f.local_count || n as usize + f.arg_count as usize == at)
            || v.is_captured && (v.var_ref_idx >= f.var_ref_count || !refs.insert(v.var_ref_idx))
        {
            return Err("invalid local metadata".into());
        }
    }
    let mut marks = vec![0u8; f.locals.len()];
    for start in 0..f.locals.len() {
        let mut path = Vec::new();
        let mut next = Some(start);
        while let Some(at) = next {
            if marks[at] == 2 {
                break;
            }
            if marks[at] == 1 {
                return Err("cyclic vardef scope chain".into());
            }
            work.charge(1)?;
            marks[at] = 1;
            path.push(at);
            next = f.locals[at]
                .scope_next
                .local_index()
                .map(|v| v as usize + f.arg_count as usize);
        }
        for at in path {
            marks[at] = 2;
        }
    }
    for c in &f.captures {
        string_budget(&c.name)?;
        if (5..=9).contains(&c.var_kind) && (!c.is_const || !c.is_lexical) {
            return Err("private class capture requires lexical constant metadata".into());
        }
        if matches!(
            c.var_kind,
            VAR_KIND_BODY_ANCHOR | VAR_KIND_BODY_ARGUMENTS_ANCHOR
        ) {
            return Err("anonymous body scope anchors cannot grant capture authority".into());
        }
        if c.var_kind > 15 {
            return Err("capture kind".into());
        }
        if module_root {
            if c.var_kind == VAR_KIND_ARGUMENTS {
                return Err("module capture cannot claim materialized arguments provenance".into());
            }
            if !matches!(c.closure_type, 6 | 7) {
                return Err("module capture kind".into());
            }
            continue;
        }
        if let Some(env) = eval_root {
            env.check_capture(c)?;
            continue;
        }
        let p = parent.ok_or("root captures require a selected-frame proof")?;
        let bound = match c.closure_type {
            0 => p.local_count,
            1 => p.arg_count,
            2 | 3 => p.captures.len() as u32,
            _ => return Err("unsupported capture type".into()),
        };
        if c.var_idx >= bound {
            return Err("capture index".into());
        }
        if c.closure_type < 2 {
            let at = if c.closure_type == 0 {
                p.arg_count + c.var_idx
            } else {
                c.var_idx
            };
            let v = p
                .locals
                .get(at as usize)
                .ok_or("capture needs parent vardefs")?;
            if matches!(
                v.var_kind,
                VAR_KIND_BODY_ANCHOR | VAR_KIND_BODY_ARGUMENTS_ANCHOR
            ) {
                return Err("anonymous body scope anchors cannot grant capture authority".into());
            }
            if v.is_lexical
                && v.name == "this".encode_utf16().collect::<Vec<_>>()
                && c.name != v.name
            {
                return Err("derived this capture cannot rename its owner capability".into());
            }
            if v.name.is_empty() && v.scope_level == 1 {
                work.charge(p.locals.len())?;
                if p.locals
                    .iter()
                    .any(|v| v.scope_next == ScopeLink::Parameters)
                {
                    return Err(
                        "anonymous parameter anchor cannot grant closure binding authority".into(),
                    );
                }
            }
            if !v.is_captured
                || c.is_lexical != v.is_lexical
                || c.is_const != v.is_const
                || (c.var_kind == VAR_KIND_ARGUMENTS || v.var_kind == VAR_KIND_ARGUMENTS)
                    && c.name != v.name
                || (c.var_kind == 3
                    || v.var_kind == 3
                    || c.var_kind == VAR_KIND_ARGUMENTS
                    || v.var_kind == VAR_KIND_ARGUMENTS
                    || (5..=9).contains(&c.var_kind)
                    || (5..=9).contains(&v.var_kind))
                    && c.var_kind != v.var_kind
            {
                return Err("capture parent slot is not marked captured".into());
            }
        } else {
            let v = &p.captures[c.var_idx as usize];
            if v.is_lexical
                && v.name == "this".encode_utf16().collect::<Vec<_>>()
                && c.name != v.name
            {
                return Err(
                    "forwarded derived this capture cannot rename its owner capability".into(),
                );
            }
            if c.is_lexical != v.is_lexical
                || c.is_const != v.is_const
                || (c.var_kind == VAR_KIND_ARGUMENTS || v.var_kind == VAR_KIND_ARGUMENTS)
                    && c.name != v.name
                || (c.var_kind == 3
                    || v.var_kind == 3
                    || c.var_kind == VAR_KIND_ARGUMENTS
                    || v.var_kind == VAR_KIND_ARGUMENTS
                    || (5..=9).contains(&c.var_kind)
                    || (5..=9).contains(&v.var_kind))
                    && c.var_kind != v.var_kind
            {
                return Err("capture binding flags mismatch".into());
            }
        }
    }
    for a in &f.atoms {
        string_budget(a)?
    }
    if let Some(s) = &f.source {
        string_budget(s)?
    }
    let parameter_head = parameter_head(f, work)?;
    let d = crate::validate_structure_schema(&f.code, l.code(), crate::production::FINAL_OPCODES)
        .map_err(limits::decode_error)?;
    let ins = d.instructions();
    let body_anchor = body_anchor(f, parameter_head, ins, work)?;
    if ins.is_empty()
        || ins.len().saturating_mul(f.local_count as usize + 1) > l.function_state_cells()
        || f.debug_locations.len() > l.max_positions
    {
        return Err("capacity: verification state budget/empty code".into());
    }
    let base_cells = ins
        .len()
        .checked_mul(f.local_count as usize + 1)
        .ok_or("capacity: verification state size overflow")?;
    budget.reserve(base_cells)?;
    let offsets: BTreeMap<_, _> = ins
        .iter()
        .enumerate()
        .map(|(j, i)| (i.offset(), j))
        .collect();
    let mut last = None;
    for p in &f.debug_locations {
        if p.line == 0
            || p.column == 0
            || p.line > i32::MAX as u32
            || p.column > i32::MAX as u32
            || !offsets.contains_key(&(p.code_offset as usize))
            || last.is_some_and(|n| n >= p.code_offset)
        {
            return Err("invalid native debug location".into());
        }
        if last.is_none() && p.code_offset != 0 {
            return Err("native debug locations must start at PC0".into());
        }
        last = Some(p.code_offset);
    }
    class_permissions(f, ins, role, work)?;
    let child_roles = class_constant_roles(f, ins, budget, work)?;
    let (constructor_arrays, argument_array_slots) =
        constructor_apply_arrays(f, ins, budget, work)?;
    work.charge(f.locals.len() + f.captures.len() + ins.len())?;
    let private_fields =
        f.locals.iter().any(|v| v.var_kind == 5) || f.captures.iter().any(|v| v.var_kind == 5);
    let private_methods = f.locals.iter().any(|v| (6..=9).contains(&v.var_kind))
        || f.captures.iter().any(|v| (6..=9).contains(&v.var_kind));
    let class_factory = ins
        .iter()
        .any(|i| matches!(i.opcode().name, "define_class" | "define_class_computed"));
    arguments_initializer(f, ins, parameter_head.is_some())?;
    new_target_initializer(f, ins, work)?;
    let mut catch_entries = BTreeMap::new();
    // lunarstorm patch: code offsets of every `catch` target, for the
    // user-catch table check below.
    let mut catch_targets = std::collections::BTreeSet::new();
    let mut scoped_protocol = false;
    for (origin, i) in ins.iter().enumerate() {
        work.charge(1)?;
        if matches!(
            i.opcode().name,
            "for_in_start"
                | "for_in_next"
                | "for_of_start"
                | "for_await_of_start"
                | "for_of_next"
                | "for_of_skip"
                | "iterator_close"
        ) {
            scoped_protocol = true;
            if f.locals.len() != (f.arg_count + f.local_count) as usize {
                return Err("iterator functions require complete vardefs".into());
            }
            if matches!(i.opcode().name, "for_of_next" | "for_of_skip") && i.operands()[0] != 0 {
                return Err("for-of next requires the proven top-record offset zero".into());
            }
        }
        if i.opcode().name == "catch" {
            if f.locals.len() != (f.arg_count + f.local_count) as usize {
                return Err("exception handlers require complete vardefs".into());
            }
            let target = i.branch_target().ok_or("catch target")?;
            if target <= (i.offset() + i.opcode().width as usize) as i128 {
                return Err("catch target must be forward and nonzero".into());
            }
            let target = usize::try_from(target).map_err(|_| "catch target range")?;
            catch_targets.insert(target);
            let target = *offsets.get(&target).ok_or("catch target boundary")?;
            budget.reserve(2)?;
            if catch_entries.insert(target, origin).is_some() {
                return Err("catch targets must have a unique handler origin".into());
            }
        }
    }
    // lunarstorm patch: the user-catch table replaces `debug_catch`'s flag
    // operand. Each entry must be a `catch` target of this function, in
    // strictly ascending order, so the engine's binary search is exact.
    if let Some(table) = &f.user_catches {
        work.charge(table.len())?;
        if table.windows(2).any(|w| w[0] >= w[1]) {
            return Err("user catch table must be strictly ascending".into());
        }
        if table.iter().any(|t| !catch_targets.contains(&(*t as usize))) {
            return Err("user catch entry is not a catch target".into());
        }
    }
    let scoped_protocol = scoped_protocol || !catch_entries.is_empty() || parameter_head.is_some();
    for (at, i) in ins.iter().enumerate() {
        work.charge(1)?;
        let n = i.opcode().name;
        if n != "catch" {
            if let Some(target) = i.branch_target() {
                let target = *offsets.get(&(target as usize)).ok_or("branch target")?;
                if catch_entries.contains_key(&target) {
                    return Err("normal branch targets a catch entry".into());
                }
            }
        }
        if catch_entries.contains_key(&(at + 1))
            && !n.starts_with("goto")
            && !(n == "strict_op" && i.operands()[4] == 5)
            && !matches!(
                n,
                "return" | "return_undef" | "return_async" | "throw" | "throw_error"
            )
        {
            return Err("normal fallthrough targets a catch entry".into());
        }
    }
    let mut marker_id = 0u32;
    let mut marker_location = 0;
    for i in ins {
        work.charge(1)?;
        effect(i)?;
        let n = i.opcode().name;
        if matches!(
            n,
            "define_method" | "define_method_computed" | "set_home_object"
        ) {
            let at = *offsets.get(&i.offset()).ok_or("method definition offset")?;
            let class_method =
                n == "set_home_object" || *i.operands().last().ok_or("method flags")? <= 2;
            if class_method {
                let previous = at
                    .checked_sub(1)
                    .and_then(|at| ins.get(at))
                    .ok_or("bound method constant")?;
                if !matches!(previous.opcode().name, "fclosure" | "fclosure8") {
                    return Err("class/home method requires its adjacent function closure".into());
                }
                let child = match f.constants.get(constant_index(previous)?) {
                    Some(Constant::Function(child)) => child,
                    _ => return Err("bound method function kind".into()),
                };
                if n != "set_home_object" && (!child.strict || child.flags.has_prototype) {
                    return Err("class methods require strict nonconstructible functions".into());
                }
                work.charge(ins.len())?;
                if ins
                    .iter()
                    .any(|branch| branch.branch_target() == Some(i.offset() as i128))
                {
                    return Err("control entry cannot bypass method home binding".into());
                }
            }
        }
        if matches!(
            n,
            "get_private_field" | "put_private_field" | "define_private_field"
        ) && !private_fields
        {
            return Err(
                "private field instruction has no qualified private field environment".into(),
            );
        }
        if n == "private_in" && !private_fields && !private_methods {
            return Err("private membership has no qualified class environment".into());
        }
        if n == "check_brand" && !private_methods {
            return Err("private method brand check has no qualified method environment".into());
        }
        if n == "add_brand"
            && (!private_methods
                || !(class_factory
                    || matches!(
                        role,
                        FunctionRole::ClassConstructor | FunctionRole::HomeBound
                    )))
        {
            return Err("private brand installation requires a class factory/initializer".into());
        }
        if n == "private_symbol" {
            if !class_factory {
                return Err("private symbols are allocated only by a class factory".into());
            }
            let at = *offsets.get(&i.offset()).ok_or("private symbol offset")?;
            let store = ins.get(at + 1).ok_or("private symbol binding")?;
            if store.opcode().name != "put_loc_check_init"
                || !f
                    .locals
                    .get(f.arg_count as usize + index(store))
                    .is_some_and(|v| v.var_kind == 5)
            {
                return Err(
                    "private symbol requires an immediate private-field identity initializer"
                        .into(),
                );
            }
            work.charge(ins.len())?;
            if ins
                .iter()
                .any(|branch| branch.branch_target() == Some(store.offset() as i128))
            {
                return Err("control entry cannot bypass private symbol allocation".into());
            }
        }
        if n == "debug_stmt" {
            // A nonempty table must locate each executable marker exactly;
            // inheriting an earlier instruction's position can invent a hit.
            // Empty tables intentionally retain unknown/stripped metadata.
            if !f.debug_locations.is_empty() {
                while marker_location < f.debug_locations.len()
                    && (f.debug_locations[marker_location].code_offset as usize) < i.offset()
                {
                    work.charge(1)?;
                    marker_location += 1;
                }
                if !f
                    .debug_locations
                    .get(marker_location)
                    .is_some_and(|p| p.code_offset as usize == i.offset())
                {
                    return Err("executable marker requires an exact debug location".into());
                }
            }
            let bits = u32::from_le_bytes(i.operands().try_into().unwrap());
            let id = (bits >> 16) & 0x7fff;
            if marker_id >= 0x7fff || id != marker_id {
                return Err("marker IDs must be bounded and contiguous".into());
            }
            marker_id += 1;
            marker_chain(f, i, work, parameter_head)?;
        }
        if parameter_head.is_some() && (n.starts_with("put_arg") || n.starts_with("set_arg")) {
            return Err("parameter expression raw argv slots are immutable".into());
        }
        if parameter_head.is_some() && n != "get_loc0_loc1" && n.contains("loc") {
            let slot = index(i);
            let v = f
                .locals
                .get(f.arg_count as usize + slot)
                .ok_or("parameter local index")?;
            if v.scope_level == 1 && v.name.is_empty() {
                if !matches!(
                    n,
                    "set_loc_uninitialized" | "put_loc_check_init" | "close_loc"
                ) {
                    return Err(
                        "anonymous parameter anchor cannot be read or assigned as a source binding"
                            .into(),
                    );
                }
                if n == "put_loc_check_init" {
                    let at = *offsets.get(&i.offset()).ok_or("anchor instruction")?;
                    work.charge(ins.len())?;
                    if at == 0
                        || ins[at - 1].opcode().name != "undefined"
                        || ins
                            .iter()
                            .any(|branch| branch.branch_target() == Some(i.offset() as i128))
                    {
                        return Err(
                            "parameter anchor initialization requires its literal undefined prefix"
                                .into(),
                        );
                    }
                }
            }
        }
        if n == "rest" {
            // Pinned OP_rest clamps the start to actual argc, then constructs
            // an array from the original argv suffix. It can allocate/throw;
            // it never reads a formal local or aliases the arguments object.
            let start = u16::from_le_bytes(i.operands().try_into().unwrap()) as u32;
            if module_root
                || eval_root.is_some()
                || f.flags.simple_parameters
                || parameter_head.is_none() && f.defined_arg_count != f.arg_count
                || start > f.arg_count
            {
                return Err(
                    "rest requires a normal non-simple parameter function and bounded start".into(),
                );
            }
        }
        // Kind 5 is the Annex B call-target ReferenceError, sloppy code only.
        if n == "throw_error"
            && i.operands()[4] > 2
            && i.operands()[4] != 4
            && !(i.operands()[4] == 5 && !f.strict)
        {
            return Err("unqualified super/iterator throw error kind".into());
        }
        if matches!(n, "get_super" | "get_super_value" | "put_super_value")
            && !f.flags.super_allowed
        {
            return Err("super property operation requires a qualified owner context".into());
        }
        if n == "init_ctor"
            && !(f.flags.derived_constructor && role == FunctionRole::ClassConstructor)
        {
            return Err("init_ctor requires a derived class constructor".into());
        }
        if n == "check_ctor_return" && !f.flags.derived_constructor {
            return Err("constructor return check requires derived completion proof".into());
        }
        if n.contains("var_ref")
            && (n.starts_with("put_") || n.starts_with("set_"))
            && f.captures.get(index(i)).is_some_and(|capture| {
                capture.is_lexical && capture.name == "this".encode_utf16().collect::<Vec<_>>()
            })
        {
            if n != "put_var_ref_check_init"
                || role != FunctionRole::LexicalSuper
                || !f.flags.super_call
            {
                return Err(
                    "captured this permits only qualified one-time lexical super initialization"
                        .into(),
                );
            }
            let at = *offsets
                .get(&i.offset())
                .ok_or("captured this initializer")?;
            constructor_result_prefix(ins, at, work)?;
        }
        let kind = f.flags.kind;
        if n == "return_async" && !module_root && kind == 0 {
            return Err("async completion requires a suspending function".into());
        }
        if matches!(n, "return" | "return_undef") && !module_root && kind != 0 {
            return Err("a suspending function completes only through return_async".into());
        }
        if matches!(n, "await" | "for_await_of_start") && !(module_root || kind & 2 != 0) {
            return Err("await requires an async function".into());
        }
        if matches!(n, "yield" | "initial_yield") && kind & 1 == 0 {
            return Err("yield requires a generator function".into());
        }
        if n == "yield_star" && kind != 1 || n == "async_yield_star" && kind != 3 {
            return Err("delegating yield does not match the generator kind".into());
        }
        if n == "dup3" || n == "iterator_get_value_done" {
            let at = *offsets.get(&i.offset()).ok_or("for-await step offset")?;
            let start = if n == "dup3" {
                Some(at)
            } else {
                at.checked_sub(4)
            };
            let step = start
                .and_then(|start| ins.get(start..start + 5))
                .filter(|step| {
                    step[0].opcode().name == "dup3"
                        && step[1].opcode().name == "drop"
                        && step[2].opcode().name == "call_method"
                        && step[2].operands() == [0, 0]
                        && step[3].opcode().name == "await"
                        && step[4].opcode().name == "iterator_get_value_done"
                });
            if let Some(step) = step {
                work.charge(ins.len())?;
                if ins.iter().any(|branch| {
                    step[1..]
                        .iter()
                        .any(|inner| branch.branch_target() == Some(inner.offset() as i128))
                }) {
                    return Err("control entry inside the for-await step".into());
                }
            } else if n == "iterator_get_value_done" {
                return Err("iterator_get_value_done outside the for-await step".into());
            }
        }
        if module_root && n == "special_object" && i.operands()[0] != 6 {
            return Err("module entrypoint cannot escape as a callable value".into());
        }
        if eval_root.is_some() {
            // Sloppy eval code in global script code declares its var/function
            // bindings on the global object; strict eval code never does.
            let global_definition =
                matches!(n, "define_var" | "check_define_var" | "define_func") && !f.strict;
            if !global_definition
                && matches!(
                    n,
                    "push_this"
                        | "special_object"
                        | "define_var"
                        | "check_define_var"
                        | "define_func"
                )
            {
                return Err("unsupported eval receiver or declaration effect".into());
            }
            if n == "put_var_ref_check_init" {
                return Err("eval cannot initialize an external binding".into());
            }
        }

        if atom_format(i.opcode().format) {
            let a = u32::from_le_bytes(i.operands()[..4].try_into().unwrap());
            if a as usize >= f.atoms.len() {
                return Err("instruction atom index".into());
            }
            if n == "strict_op" {
                let operation = i.operands()[4];
                if operation > 6 {
                    return Err("unknown strict operation selector".into());
                }
                if operation != 3 && !f.atoms[a as usize].is_empty() {
                    return Err(
                        "strict property operation requires its canonical unused atom".into(),
                    );
                }
                if operation == 2 && !f.flags.super_allowed {
                    return Err("strict super store requires a qualified owner context".into());
                }
            }
            if n == "delete_var" {
                // The pinned runtime deletes only a global binding here; it does
                // not perform lexical resolution or consult the selected frame.
                // Strict identifier deletion is an early syntax error, while
                // deleting a resolved eval binding must instead produce false.
                if f.strict {
                    return Err("strict identifier deletion is forbidden".into());
                }
                if eval_root.is_some_and(|env| env.binding(&f.atoms[a as usize]).is_some()) {
                    return Err("eval identifier deletion cannot target a frame binding".into());
                }
            }
        }
        if n == "push_const" || n == "push_const8" || n == "fclosure" || n == "fclosure8" {
            let j = if i.operands().len() == 1 {
                usize::from(i.operands()[0])
            } else {
                u32::from_le_bytes(i.operands()[..4].try_into().unwrap()) as usize
            };
            let c = f.constants.get(j).ok_or("constant index")?;
            if n.starts_with("fclosure") && !matches!(c, Constant::Function(_)) {
                return Err("constant instruction kind".into());
            }
        }
        let bound = if n.contains("loc") && n != "get_loc0_loc1" {
            Some(f.local_count)
        } else if n.starts_with("get_arg") || n.starts_with("put_arg") || n.starts_with("set_arg") {
            Some(f.arg_count)
        } else if n.contains("var_ref") {
            Some(f.captures.len() as u32)
        } else {
            None
        };
        if let Some(bound) = bound {
            if index(i) >= bound as usize {
                return Err(format!("{n} index"));
            }
        }
        // The VM reads the disposable value at the slot and its method at the next one.
        if matches!(n, "using_dispose" | "using_dispose_async") {
            for slot in [index(i), index(i) + 1] {
                let v = (slot < f.local_count as usize)
                    .then(|| f.locals.get(f.arg_count as usize + slot))
                    .flatten()
                    .ok_or("using slot index")?;
                if !v.name.is_empty() || v.is_lexical || v.is_captured || v.var_kind != 0 {
                    return Err("using slots must be anonymous temporaries".into());
                }
            }
        }
        if n == "set_loc_uninitialized"
            && !f
                .locals
                .get(f.arg_count as usize + index(i))
                .is_some_and(|v| v.is_lexical)
        {
            return Err("lexical sentinel requires lexical vardef".into());
        }
        if n == "close_loc" {
            let v = f
                .locals
                .get(f.arg_count as usize + index(i))
                .ok_or("close_loc needs vardefs")?;
            if !v.is_captured || v.var_ref_idx >= f.var_ref_count {
                return Err("close_loc needs captured slot".into());
            }
        }
        if n == "get_loc0_loc1" && f.local_count < 2 {
            return Err("paired local index".into());
        }
        if n.contains("var_ref")
            && (n.starts_with("put_") || n.starts_with("set_"))
            && f.captures[index(i)].is_const
        {
            let c = &f.captures[index(i)];
            if !(module_root && c.closure_type == 6 && n == "put_var_ref_check_init") {
                return Err("write to constant capture".into());
            }
        }

        if n.contains("var_ref")
            && (n.starts_with("put_") || n.starts_with("set_"))
            && eval_root.is_some_and(|env| env.readonly_hidden(&f.captures[index(i)].name))
        {
            return Err("eval cannot overwrite a hidden context binding".into());
        }
        if eval_root.is_some()
            && n.contains("var_ref")
            && (n.starts_with("put_") || n.starts_with("set_"))
            && f.captures[index(i)].var_kind == 4
        {
            return Err("eval cannot write an immutable function name".into());
        }
        if n.contains("var_ref") && !n.contains("check") && f.captures[index(i)].is_lexical {
            return Err("lexical capture needs checked access".into());
        }
        if matches!(n, "define_var" | "check_define_var" | "define_func")
            && i.operands()[4] & !0xc7 != 0
        {
            return Err("global definition flags".into());
        }
    }
    // Reserve ordinary facts and every sparse handler/baseline before storing
    // them. The same allowance follows all nested functions in the graph.
    let initial: Vec<u8> = (0..f.local_count)
        .map(|n| {
            if f.locals.is_empty()
                || f.locals[(f.arg_count + n) as usize].is_lexical
                || f.locals[(f.arg_count + n) as usize].var_kind == 3
            {
                0
            } else {
                2
            }
        })
        .collect();
    let mut states: Vec<Option<FrameState>> = vec![None; ins.len()];
    states[0] = Some(FrameState {
        stack: 0,
        locals: initial,
        activity: vec![1; f.local_count as usize],
        handlers: Vec::new(),
        iterators: Vec::new(),
        apply_array: None,
        pending_exception: None,
        started: f.flags.kind & 1 == 0,
    });
    let mut baselines: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    let mut queue = VecDeque::from([0]);
    let mut queued = vec![false; ins.len()];
    queued[0] = true;
    while let Some(j) = queue.pop_front() {
        queued[j] = false;
        let entry = states[j].as_ref().unwrap();
        work.charge(
            1 + 2 * f.local_count as usize
                + 3 * entry.handlers.len()
                + 3 * entry.iterators.len()
                + 2 * usize::from(entry.apply_array.is_some()),
        )?;
        let mut state = entry.clone();
        let i = &ins[j];
        let n = i.opcode().name;
        let completion = Completion::of(i, &offsets)?;
        if n == "apply" && u16::from_le_bytes(i.operands().try_into().unwrap()) == 1 {
            if !state
                .apply_array
                .is_some_and(|proof| proof.target == j && proof.depth + 1 == state.stack)
            {
                return Err(
                    "constructor apply has no retained non-null argument-array proof".into(),
                );
            }
            state.apply_array = None;
        }
        let (pop, push) = effect(i)?;
        if n == "check_ctor_return"
            && (!state.handlers.is_empty()
                || !state.iterators.is_empty()
                || state.apply_array.is_some()
                || state.pending_exception.is_some())
        {
            return Err("derived return check requires completed handler/iterator cleanup".into());
        }
        if state.stack < pop {
            return Err(format!("stack underflow at {}", i.offset()));
        }
        let native_iterator = matches!(
            n,
            "for_in_next" | "for_of_next" | "for_of_skip" | "iterator_close"
        );
        let dropping_enum = n == "drop"
            && state.iterators.last().is_some_and(|record| {
                record.kind == IteratorKind::ForIn && record.end() == state.stack
            });
        // Dropping the private catch sentinel of a for-of record leaves the
        // iterator and next method as ordinary values that no unwind closes.
        let dissolving = n == "drop"
            && state.iterators.last().is_some_and(|record| {
                record.kind != IteratorKind::ForIn && record.end() == state.stack
            });
        // The for-await step copies its live record once; the static check
        // pinned the following drop/call_method/await/iterator_get_value_done.
        let stepping = n == "dup3"
            && state.iterators.last().is_some_and(|record| {
                record.kind == IteratorKind::ForAwait && record.end() == state.stack
            });
        if n == "iterator_get_value_done"
            && !state.iterators.last().is_some_and(|record| {
                record.kind == IteratorKind::ForAwait && record.end() + 1 == state.stack
            })
        {
            return Err("iterator_get_value_done requires its live for-await record".into());
        }
        if n == "initial_yield" {
            if state.started {
                return Err("initial_yield must run exactly once".into());
            }
            state.started = true;
        } else if !state.started
            && matches!(
                n,
                "yield" | "yield_star" | "async_yield_star" | "await" | "return_async"
            )
        {
            return Err("generator suspends or completes before initial_yield".into());
        }
        if native_iterator {
            let expected = if n == "for_in_next" {
                IteratorKind::ForIn
            } else {
                IteratorKind::ForOf
            };
            if !state
                .iterators
                .last()
                .is_some_and(|record| record.kind == expected && record.end() == state.stack)
            {
                return Err("iterator opcode requires its exact live top record".into());
            }
        }
        work.charge(state.iterators.len())?;
        if !native_iterator
            && !dropping_enum
            && !dissolving
            && !stepping
            && state
                .iterators
                .iter()
                .any(|record| record.end() > state.stack - pop && record.depth < state.stack)
        {
            return Err("ordinary instruction exposes or rearranges an iterator record".into());
        }
        let popping_handler = n == "drop"
            && state
                .handlers
                .last()
                .is_some_and(|h| h.depth + 1 == state.stack);
        if state
            .handlers
            .last()
            .is_some_and(|h| h.depth >= state.stack - pop)
            && !popping_handler
        {
            return Err("ordinary instruction consumes or rearranges a private catch token".into());
        }
        let mut initializing_catch = None;
        if let Some(origin) = state.pending_exception {
            let baseline = baselines.get(&origin).ok_or("missing exception baseline")?;
            if n == "close_loc" {
                let k = index(i);
                let v = &f.locals[f.arg_count as usize + k];
                if !scoped(v) || baseline[k] != 1 {
                    return Err("exception cleanup closes an outer/non-scoped slot".into());
                }
            } else {
                for (k, activity) in state.activity.iter().enumerate() {
                    work.charge(1)?;
                    if scoped(&f.locals[f.arg_count as usize + k]) && *activity != baseline[k] {
                        return Err(
                            "exception entry has undischarged lexical close obligations".into()
                        );
                    }
                }
                if n == "drop" {
                    state.pending_exception = None;
                } else if matches!(
                    n,
                    "put_loc" | "put_loc8" | "put_loc0" | "put_loc1" | "put_loc2" | "put_loc3"
                ) {
                    let k = index(i);
                    let v = f
                        .locals
                        .get(f.arg_count as usize + k)
                        .ok_or("catch parameter vardef")?;
                    match exception_sink(v, k)? {
                        ExceptionSink::CatchBinding(slot) => {
                            if baseline[slot] != 1 || state.activity[slot] != 1 {
                                return Err(
                                    "exception value requires an inactive native catch binding"
                                        .into(),
                                );
                            }
                            initializing_catch = Some(slot);
                        }
                        ExceptionSink::CompletionTemporary => {}
                    }
                    state.pending_exception = None;
                } else {
                    return Err("exception entry requires straight close prefix then typed exception store/drop".into());
                }
            }
        }
        if n == "debug_stmt" {
            let chain = marker_chain(f, i, work, parameter_head)?;
            let mut expected = vec![false; f.local_count as usize];
            for k in chain {
                expected[k] = true;
            }
            for (k, value) in state.activity.iter().enumerate() {
                if scoped(&f.locals[f.arg_count as usize + k])
                    && *value != if expected[k] { 2 } else { 1 }
                {
                    return Err("marker scope does not match lexical activity on every path".into());
                }
            }
        }
        if matches!(
            n,
            "fclosure" | "fclosure8" | "define_class" | "define_class_computed"
        ) {
            let constant = if matches!(n, "define_class" | "define_class_computed") {
                constant_index(&ins[j - 1])?
            } else {
                constant_index(i)?
            };
            let child = match &f.constants[constant] {
                Constant::Function(child) => child,
                _ => return Err("closure constant kind".into()),
            };
            for (at, capture) in child.captures.iter().enumerate() {
                work.charge(1)?;
                if capture.closure_type == 0 {
                    let slot = capture.var_idx as usize;
                    let v = f
                        .locals
                        .get(f.arg_count as usize + slot)
                        .ok_or("closure parent local")?;
                    if (v.var_kind == 3 || scoped_protocol && scoped(v))
                        && capture_is_used(child, at, work)?
                        && state.activity.get(slot) != Some(&2)
                    {
                        return Err("closure accesses an inactive scoped binding".into());
                    }
                }
            }
        }
        if n == "iterator_close" || dropping_enum || dissolving {
            state.iterators.pop().ok_or("iterator cleanup record")?;
        }
        // Build exceptional transfer from instruction-entry facts. None of the
        // admitted throwing operations initializes a local before its failure.
        if may_throw(n) {
            if let Some(handler) = state.handlers.last().copied() {
                let baseline = baselines
                    .get(&handler.origin)
                    .ok_or("missing handler baseline")?;
                for (k, value) in state.activity.iter().enumerate() {
                    work.charge(1)?;
                    if !f.locals.is_empty()
                        && scoped(&f.locals[f.arg_count as usize + k])
                        && baseline[k] == 2
                        && *value != 2
                    {
                        return Err(
                            "throwing operation while an outer handler scope is detached".into(),
                        );
                    }
                }
                work.charge(
                    1 + 2 * state.locals.len()
                        + 3 * state.handlers.len()
                        + 3 * state.iterators.len()
                        + 2 * usize::from(state.apply_array.is_some()),
                )?;
                let mut exceptional = state.clone();
                exceptional.handlers.pop();
                exceptional.apply_array = None;
                // Native unwind visits every zero sentinel above this ordinary
                // handler, closes that iterator with exception_pending=true,
                // discards its next/object, then continues. Return() failures
                // preserve the original body exception. for_of_next/skip failure
                // has already replaced its iterator object with undefined, so
                // that particular record performs no user-visible close.
                // Records below the catch tag survive exactly as they were.
                work.charge(exceptional.iterators.len())?;
                exceptional
                    .iterators
                    .retain(|record| record.end() <= handler.depth);
                exceptional.stack = handler.depth + 1;
                exceptional.pending_exception = Some(handler.origin);
                merge_state(
                    handler.target,
                    &exceptional,
                    &mut states,
                    &mut queue,
                    &mut queued,
                    budget,
                    work,
                )?;
            }
        }
        state.stack = state.stack - pop + push;
        if state.stack > f.stack_size as usize {
            return Err("declared stack too small".into());
        }
        if let Some(&(target, slot)) = constructor_arrays.get(&j) {
            if state.apply_array.is_some() || state.locals.get(slot) != Some(&3) {
                return Err(
                    "constructor argument array was not definitely created on every path".into(),
                );
            }
            state.apply_array = Some(ApplyArray {
                target,
                depth: state.stack - 1,
            });
        }
        if matches!(n, "for_in_start" | "for_of_start" | "for_await_of_start") {
            let kind = match n {
                "for_in_start" => IteratorKind::ForIn,
                "for_of_start" => IteratorKind::ForOf,
                _ => IteratorKind::ForAwait,
            };
            let width = if kind == IteratorKind::ForIn { 1 } else { 3 };
            work.charge(3 * (state.iterators.len() + 1))?;
            if budget.remaining < 3 * (state.iterators.len() + 1) {
                return Err("capacity: live iterator state budget".into());
            }
            state
                .iterators
                .try_reserve_exact(1)
                .map_err(|_| "capacity: live iterator allocation")?;
            state.iterators.push(IteratorRecord {
                origin: j,
                depth: state.stack - width,
                kind,
            });
        }
        if matches!(n, "catch" | "debug_catch") {
            // Installation itself cannot throw. A synthetic exceptional entry
            // still verifies every handler body when its try contains no
            // currently throwing operation; it is not a normal control edge.
            work.charge(state.activity.len())?;
            if state.activity.iter().enumerate().any(|(k, value)| {
                !f.locals.is_empty()
                    && scoped(&f.locals[f.arg_count as usize + k])
                    && *value != 1
                    && *value != 2
            }) {
                return Err("handler installation requires exact scoped activity".into());
            }
            for handler in &state.handlers {
                let outer = baselines
                    .get(&handler.origin)
                    .ok_or("outer handler baseline")?;
                for (k, value) in state.activity.iter().enumerate() {
                    work.charge(1)?;
                    if scoped(&f.locals[f.arg_count as usize + k]) && outer[k] == 2 && *value != 2 {
                        return Err("handler installation while an outer scope is detached".into());
                    }
                }
            }
            work.charge(state.activity.len())?;
            match baselines.get(&j) {
                Some(old) if old != &state.activity => {
                    return Err("handler lexical baseline mismatch".into());
                }
                Some(_) => {}
                None => {
                    budget.reserve(state.activity.len())?;
                    work.charge(state.activity.len())?;
                    baselines.insert(j, state.activity.clone());
                }
            }
            let target = match completion {
                Completion::EnterHandler { target } => target,
                _ => return Err("catch completion kind".into()),
            };
            work.charge(
                1 + 2 * state.locals.len()
                    + 3 * state.handlers.len()
                    + 3 * state.iterators.len()
                    + 2 * usize::from(state.apply_array.is_some()),
            )?;
            let mut synthetic = state.clone();
            synthetic.pending_exception = Some(j);
            merge_state(
                target,
                &synthetic,
                &mut states,
                &mut queue,
                &mut queued,
                budget,
                work,
            )?;
            work.charge(3 * (state.handlers.len() + 1))?;
            if budget.remaining < 3 * (state.handlers.len() + 1) {
                return Err("capacity: live handler state budget".into());
            }
            state
                .handlers
                .try_reserve_exact(1)
                .map_err(|_| "capacity: live handler allocation")?;
            state.handlers.push(Handler {
                origin: j,
                target,
                depth: state.stack - 1,
            });
        } else if popping_handler {
            let handler = state.handlers.pop().ok_or("handler drop")?;
            let baseline = baselines
                .get(&handler.origin)
                .ok_or("handler drop baseline")?;
            for (k, activity) in state.activity.iter().enumerate() {
                work.charge(1)?;
                if !f.locals.is_empty()
                    && scoped(&f.locals[f.arg_count as usize + k])
                    && baseline[k] == 1
                    && *activity != 1
                {
                    return Err(
                        "normal handler exit leaves descendant scope references open".into(),
                    );
                }
            }
        }
        let local_indices = if n == "get_loc0_loc1" {
            vec![0, 1]
        } else if n.contains("loc") {
            vec![index(i)]
        } else {
            Vec::new()
        };
        for k in local_indices {
            if body_anchor == Some(k) {
                if !matches!(n, "set_loc_uninitialized" | "close_loc") {
                    return Err("anonymous body anchor has no value authority".into());
                }
                if n == "set_loc_uninitialized" {
                    if state.locals[k] != 0 {
                        return Err("body anchor cannot be reentered".into());
                    }
                    for (slot, parameter) in f.locals[f.arg_count as usize..].iter().enumerate() {
                        work.charge(1)?;
                        if parameter.scope_level == 1
                            && (state.locals[slot] < 2 || state.activity[slot] != 2)
                        {
                            return Err(
                                "body scope cannot activate before all parameters are initialized"
                                    .into(),
                            );
                        }
                    }
                }
            }
            if parameter_head.is_some()
                && f.locals[f.arg_count as usize + k].scope_level == 1
                && f.locals[f.arg_count as usize + k].name.is_empty()
                && n.starts_with("get_loc")
            {
                return Err("anonymous parameter anchor has no read authority".into());
            }
            let scoped_access = f
                .locals
                .get(f.arg_count as usize + k)
                .is_some_and(|v| v.var_kind == 3 || scoped_protocol && scoped(v));
            if scoped_access
                && n != "set_loc_uninitialized"
                && (n.starts_with("get_loc")
                    || n.starts_with("put_loc")
                    || n.starts_with("set_loc"))
                && initializing_catch != Some(k)
                && state.activity[k] != 2
            {
                return Err("ordinary access to an inactive scoped binding".into());
            }
            if parameter_head.is_some() && f.locals[f.arg_count as usize + k].scope_level == 1 {
                if n == "set_loc_uninitialized" && state.locals[k] != 0 {
                    return Err("parameter TDZ cells may be created only once".into());
                }
                if n == "put_loc_check_init" {
                    for (slot, v) in f.locals[f.arg_count as usize..].iter().enumerate() {
                        work.charge(1)?;
                        if v.scope_level == 1
                            && (state.locals[slot] == 0 || slot < k && state.locals[slot] < 2)
                        {
                            return Err("parameter initialization requires a complete TDZ and left-to-right initialization".into());
                        }
                    }
                    if state.locals[k] != 1 {
                        return Err("parameter binding is not awaiting initialization".into());
                    }
                } else if (n.starts_with("put_loc") || n.starts_with("set_loc"))
                    && n != "set_loc_uninitialized"
                    && n != "put_loc_check"
                    && state.locals[k] != 2
                {
                    return Err("parameter write bypasses its TDZ check".into());
                }
            }
            if n.starts_with("get_loc")
                && state.locals[k] < if n == "get_loc_check" { 1 } else { 2 }
            {
                return Err("uninitialized local read".into());
            }
            if n == "set_loc_uninitialized" {
                state.locals[k] = 1;
                state.activity[k] = 2;
            } else if n.starts_with("put_loc") || n.starts_with("set_loc") {
                state.locals[k] = if argument_array_slots.contains(&k)
                    && plain_local_put(n)
                    && j > 0
                    && zero_array(&ins[j - 1])
                {
                    3
                } else {
                    2
                };
            }
            if initializing_catch == Some(k) {
                state.activity[k] = 2;
            }
            if n == "close_loc" {
                state.activity[k] = 1;
            }
        }
        if matches!(completion, Completion::Return | Completion::Throw) {
            if matches!(completion, Completion::Return) {
                for (k, activity) in state.activity.iter().enumerate() {
                    work.charge(1)?;
                    if f.locals
                        .get(f.arg_count as usize + k)
                        .is_some_and(|v| v.var_kind == 3)
                        && *activity != 1
                    {
                        return Err("return leaves a catch binding reference open".into());
                    }
                }
            }
            if matches!(completion, Completion::Return)
                && (state.stack != 0
                    || !state.handlers.is_empty()
                    || !state.iterators.is_empty()
                    || state.apply_array.is_some()
                    || state.pending_exception.is_some())
            {
                return Err("return stack/handler cleanup incomplete".into());
            }
            continue;
        }
        let mut successors = Vec::new();
        let mut with_taken = None;
        let fallthrough = match completion {
            Completion::Normal | Completion::EnterHandler { .. } => true,
            Completion::Branch {
                target,
                conditional,
            } => {
                successors.push(target);
                conditional
            }
            Completion::With { target, delta } => {
                let mut taken = state.clone();
                taken.stack = taken
                    .stack
                    .checked_add_signed(delta)
                    .ok_or("with lookup stack underflow")?;
                with_taken = Some((target, taken));
                true
            }
            Completion::Return | Completion::Throw => {
                return Err("terminal completion fell through".into());
            }
        };
        if fallthrough {
            if j + 1 == ins.len() {
                return Err("reachable fallthrough".into());
            }
            successors.push(j + 1);
        }
        for next in successors {
            if catch_entries.contains_key(&next) {
                return Err("normal control flow enters a catch entry".into());
            }
            merge_state(
                next,
                &state,
                &mut states,
                &mut queue,
                &mut queued,
                budget,
                work,
            )?;
        }
        if let Some((next, taken)) = with_taken {
            if catch_entries.contains_key(&next) {
                return Err("normal control flow enters a catch entry".into());
            }
            merge_state(
                next,
                &taken,
                &mut states,
                &mut queue,
                &mut queued,
                budget,
                work,
            )?;
        }
    }
    drop(states);
    drop(offsets);
    drop(d);
    for (index, c) in f.constants.iter().enumerate() {
        match c {
            Constant::Function(child) => {
                let child_role = child_roles
                    .get(&index)
                    .copied()
                    .unwrap_or(FunctionRole::Ordinary);
                verify_function(
                    child,
                    Some(f),
                    RootContext::Ordinary,
                    l,
                    work,
                    budget,
                    child_role,
                )?
            }
            Constant::String(s) => string_budget(s)?,
            Constant::BigInt(bytes)
                if bytes == &[0]
                    || bytes.len() > 1
                        && (bytes[bytes.len() - 1] == 0 && bytes[bytes.len() - 2] & 128 == 0
                            || bytes[bytes.len() - 1] == 255
                                && bytes[bytes.len() - 2] & 128 != 0) =>
            {
                return Err("noncanonical BigInt".into());
            }
            Constant::TemplateObject { cooked, raw } => {
                if raw.is_empty() || cooked.len() != raw.len() || raw.len() > 65535 {
                    return Err("template object shape".into());
                }
                for s in raw.iter().chain(cooked.iter().flatten()) {
                    string_budget(s)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn verify_module_with_limits(m: &ModuleImage, l: &ImageLimits) -> Result<usize, String> {
    let mut work = limits::preflight(&m.function, Some(m), l)?;
    let f = &m.function;
    if !f.strict || f.flags.has_prototype || f.arg_count != 0 || f.defined_arg_count != 0 {
        return Err("module entrypoint/TLA contract".into());
    }
    for n in [
        m.required_modules.len(),
        m.exports.len(),
        m.star_exports.len(),
        m.imports.len(),
    ] {
        if n > 65535 {
            return Err("capacity: module table budget".into());
        }
    }
    let mut budget = l.max_graph_bytes;
    let mut charge = |s: &[u16]| -> Result<(), String> {
        string_budget(s)?;
        budget = budget
            .checked_sub(s.len() * 2)
            .ok_or("capacity: module text budget")?;
        Ok(())
    };
    charge(&m.name)?;
    if m.name.is_empty() || m.name.contains(&0) {
        return Err("module name".into());
    }
    // lunarstorm patch: each request carries its import attributes; keys are
    // unique within a request and specifiers stay unique across requests.
    if !m.required_attributes.is_empty() && m.required_attributes.len() != m.required_modules.len()
    {
        return Err("module request attributes".into());
    }
    let no_attributes = Vec::new();
    let mut names = BTreeSet::new();
    for (i, n) in m.required_modules.iter().enumerate() {
        charge(n)?;
        let attributes = m.required_attributes.get(i).unwrap_or(&no_attributes);
        let mut keys = BTreeSet::new();
        for (k, v) in attributes {
            charge(k)?;
            charge(v)?;
            if !keys.insert(k) {
                return Err("duplicate module request attribute".into());
            }
        }
        if n.is_empty() || n.contains(&0) || !names.insert(n) {
            return Err("module dependency name".into());
        }
    }
    let mut exports = BTreeSet::new();
    for e in &m.exports {
        let name = match e {
            ModuleExport::Local { capture, name } => {
                if *capture as usize >= f.captures.len() {
                    return Err("module local export capture".into());
                }
                name
            }
            ModuleExport::Indirect {
                module,
                import,
                name,
            } => {
                if *module as usize >= m.required_modules.len() {
                    return Err("indirect export module".into());
                }
                charge(import)?;
                name
            }
        };
        charge(name)?;
        if !exports.insert(name) {
            return Err("duplicate module export".into());
        }
    }
    let mut stars = BTreeSet::new();
    for i in &m.star_exports {
        if *i as usize >= m.required_modules.len() || !stars.insert(i) {
            return Err("star export module".into());
        }
    }
    let mut imported = BTreeSet::new();
    for (at, i) in m.imports.iter().enumerate() {
        charge(&i.name)?;
        if i.module as usize >= m.required_modules.len() || !imported.insert(i.capture) {
            return Err("import module or duplicate slot".into());
        }
        let c = f.captures.get(i.capture as usize).ok_or("import capture")?;
        let namespace = i.name == [b'*' as u16];
        if c.closure_type != if namespace { 6 } else { 7 }
            || c.var_idx != at as u32
            || !c.is_const
            || !c.is_lexical
        {
            return Err("import capture contract".into());
        }
    }
    let local_decls = f
        .captures
        .iter()
        .enumerate()
        .filter(|(at, c)| c.closure_type == 6 && !imported.contains(&(*at as u32)))
        .count();
    let mut decls = BTreeSet::new();
    let mut bindings = BTreeSet::new();
    for (at, c) in f.captures.iter().enumerate() {
        if !bindings.insert(&c.name) {
            return Err("duplicate module binding".into());
        }
        match c.closure_type {
            6 => {
                if !imported.contains(&(at as u32))
                    && (c.var_idx as usize >= local_decls || !decls.insert(c.var_idx))
                {
                    return Err("module declaration index".into());
                }
            }
            7 => {
                if !imported.contains(&(at as u32)) {
                    return Err("unbound module import slot".into());
                }
            }
            _ => return Err("module capture type".into()),
        }
    }
    let mut budget = StateBudget::new(l);
    verify_function(
        f,
        None,
        RootContext::Module,
        l,
        &mut work,
        &mut budget,
        FunctionRole::Ordinary,
    )?;
    let decoded = crate::decode_schema(&f.code, l.code(), crate::production::FINAL_OPCODES)
        .map_err(limits::decode_error)?;
    for i in decoded.instructions() {
        let n = i.opcode().name;
        if n.contains("var_ref")
            && (n.starts_with("put_") || n.starts_with("set_"))
            && imported.contains(&(index(i) as u32))
        {
            return Err("write to imported module binding".into());
        }
    }
    verify_module_phases(f, l, &mut work)?;
    Ok(l.max_work - work.remaining())
}
fn verify_module_phases(
    f: &FunctionImage,
    l: &ImageLimits,
    work: &mut limits::Work,
) -> Result<(), String> {
    let decoded = crate::decode_schema(&f.code, l.code(), crate::production::FINAL_OPCODES)
        .map_err(limits::decode_error)?;
    let ins = decoded.instructions();
    if ins.len() < 4
        || ins[0].opcode().name != "push_this"
        || !matches!(ins[1].opcode().name, "if_false" | "if_false8")
    {
        return Err("module requires initialization/evaluation phase prologue".into());
    }
    let offsets: BTreeMap<_, _> = ins
        .iter()
        .enumerate()
        .map(|(at, i)| (i.offset(), at))
        .collect();
    let entry = *offsets
        .get(&(ins[1].branch_target().ok_or("module phase target")? as usize))
        .ok_or("module phase boundary")?;
    if entry <= 2 {
        return Err("module phase ordering".into());
    }
    for (start, evaluation) in [(2, false), (entry, true)] {
        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::from([start]);
        while let Some(at) = queue.pop_front() {
            work.charge(1)?;
            if !seen.insert(at) {
                continue;
            }
            let i = &ins[at];
            let n = i.opcode().name;
            if matches!(n, "return" | "return_undef" | "return_async") {
                if n != if evaluation {
                    "return_async"
                } else {
                    "return_undef"
                } {
                    return Err("module phase completion ABI".into());
                }
                continue;
            }
            if matches!(n, "throw" | "throw_error") || n == "strict_op" && i.operands()[4] == 5 {
                continue;
            }
            if let Some(target) = i.branch_target() {
                queue.push_back(
                    *offsets
                        .get(&(target as usize))
                        .ok_or("module phase branch")?,
                )
            }
            if !n.starts_with("goto") {
                if at + 1 >= ins.len() {
                    return Err("module phase fallthrough".into());
                }
                queue.push_back(at + 1)
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod marker_decode_tests {
    use super::*;
    use crate::production::ids;
    #[test]
    fn recomputed_native_checksum_does_not_prove_marker_activity() {
        let f = FunctionImage {
            code: vec![ids::debug_stmt, 1, 0, 0, 0, ids::return_undef],
            local_count: 1,
            var_ref_count: 1,
            locals: vec![VarDef {
                name: vec![b'x' as u16],
                scope_level: 1,
                is_lexical: true,
                is_captured: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let bytes = super::super::format::encode(&f).unwrap();
        assert!(super::super::decode_and_verify(&bytes).is_err());
    }
}

#[cfg(test)]
mod arguments_decode_tests {
    use super::*;
    use crate::production::ids;
    #[test]
    fn malformed_mapped_arguments_never_reach_native_vardef_access() {
        let mut f = FunctionImage {
            code: vec![
                ids::special_object,
                1,
                ids::put_loc,
                0,
                0,
                ids::return_undef,
            ],
            arg_count: 1,
            defined_arg_count: 1,
            local_count: 1,
            var_ref_count: 2,
            stack_size: 1,
            locals: vec![
                VarDef {
                    name: vec![b'a' as u16],
                    is_captured: true,
                    ..Default::default()
                },
                VarDef {
                    name: "arguments".encode_utf16().collect(),
                    is_captured: true,
                    var_ref_idx: 1,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        f.flags.arguments_allowed = true;
        super::super::decode_and_verify(&super::super::format::encode(&f).unwrap()).unwrap();
        f.locals.clear();
        assert!(
            super::super::decode_and_verify(&super::super::format::encode(&f).unwrap()).is_err()
        );
    }
}
