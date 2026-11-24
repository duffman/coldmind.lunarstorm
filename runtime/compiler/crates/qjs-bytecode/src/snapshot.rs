//! Owned final instructions and their compiler provenance, separate from serialized objects.
#[derive(Clone, Debug, PartialEq)]
pub enum Constant {
    Number(f64),
    Bool(bool),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provenance {
    QuickJsFinalized,
    RustFinalized,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Position {
    pub code_offset: u32,
    pub source_start: u32,
    pub source_end: u32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub code: Vec<u8>,
    pub constants: Vec<Constant>,
    pub local_count: u32,
    pub arg_count: u32,
    pub stack_size: u32,
    pub source_len: u32,
    pub positions: Vec<Position>,
    pub function_name: String,
    pub provenance: Provenance,
    pub target_fingerprint: String,
}
